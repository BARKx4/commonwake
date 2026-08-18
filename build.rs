use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const BUNDLE_FILE: &str = "commonwake-source.bundle";
const MAX_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;

fn main() {
    println!("cargo:rerun-if-env-changed=COMMONWAKE_SOURCE_BUNDLE");
    println!("cargo:rerun-if-env-changed=COMMONWAKE_SOURCE_REVISION");
    println!("cargo:rerun-if-env-changed=COMMONWAKE_SOURCE_EXACT");
    println!("cargo:rerun-if-env-changed=COMMONWAKE_SOURCE_PROVENANCE");
    println!("cargo:rerun-if-env-changed=COMMONWAKE_SOURCE_DEFAULT_REF");

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    emit_rerun_paths(&manifest_dir);
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let output = out_dir.join(BUNDLE_FILE);
    let _ = fs::remove_file(&output);

    let supplied = env::var_os("COMMONWAKE_SOURCE_BUNDLE")
        .map(PathBuf::from)
        .or_else(|| {
            let candidate = manifest_dir
                .join(".commonwake-build")
                .join("commonwake.bundle");
            candidate.is_file().then_some(candidate)
        });

    let metadata = if let Some(bundle) = supplied {
        copy_supplied_bundle(&bundle, &output)
    } else {
        create_bundle_from_checkout(&manifest_dir, &output)
    }
    .unwrap_or_else(|error| {
        panic!(
            "Commonwake builds must carry a reconstructable source bundle: {error}. \
             Build from a Git checkout or set COMMONWAKE_SOURCE_BUNDLE."
        )
    });

    let size = fs::metadata(&output)
        .unwrap_or_else(|error| panic!("could not inspect generated source bundle: {error}"))
        .len();
    assert!(size > 0, "generated source bundle is empty");
    assert!(
        size <= MAX_BUNDLE_BYTES,
        "generated source bundle exceeds the 256 MiB self-source limit"
    );

    println!(
        "cargo:rustc-env=COMMONWAKE_SOURCE_REVISION={}",
        metadata.revision
    );
    println!("cargo:rustc-env=COMMONWAKE_SOURCE_EXACT={}", metadata.exact);
    println!(
        "cargo:rustc-env=COMMONWAKE_SOURCE_PROVENANCE={}",
        metadata.provenance
    );
    println!(
        "cargo:rustc-env=COMMONWAKE_SOURCE_DEFAULT_REF={}",
        metadata.default_ref
    );
}

struct SourceMetadata {
    revision: String,
    exact: bool,
    provenance: String,
    default_ref: String,
}

fn copy_supplied_bundle(bundle: &Path, output: &Path) -> Result<SourceMetadata, String> {
    if !bundle.is_file() {
        return Err(format!(
            "supplied bundle {} does not exist",
            bundle.display()
        ));
    }
    fs::copy(bundle, output).map_err(|error| {
        format!(
            "could not copy supplied bundle {} to {}: {error}",
            bundle.display(),
            output.display()
        )
    })?;

    let parent = bundle
        .parent()
        .ok_or_else(|| "supplied source bundle has no parent directory".to_owned())?;
    let revision = env_or_metadata("COMMONWAKE_SOURCE_REVISION", parent.join("revision"))?;
    let exact = env_or_metadata("COMMONWAKE_SOURCE_EXACT", parent.join("exact"))?
        .parse::<bool>()
        .map_err(|_| "source exactness must be true or false".to_owned())?;
    let provenance = env_or_metadata("COMMONWAKE_SOURCE_PROVENANCE", parent.join("provenance"))?;
    let default_ref = env_or_metadata("COMMONWAKE_SOURCE_DEFAULT_REF", parent.join("default-ref"))?;
    validate_metadata(&revision, &provenance, &default_ref)?;

    Ok(SourceMetadata {
        revision,
        exact,
        provenance,
        default_ref,
    })
}

fn create_bundle_from_checkout(
    manifest_dir: &Path,
    output: &Path,
) -> Result<SourceMetadata, String> {
    let revision = git_output(manifest_dir, &["rev-parse", "HEAD"])?;
    let has_main = git_status(
        manifest_dir,
        &["show-ref", "--verify", "--quiet", "refs/heads/main"],
    );
    let default_ref = if has_main { "refs/heads/main" } else { "HEAD" };

    let mut command = Command::new("git");
    command
        .current_dir(manifest_dir)
        .args(["bundle", "create"])
        .arg(output)
        .arg("HEAD");
    if has_main {
        command.arg("refs/heads/main");
    }
    command.arg("--tags");
    let result = command
        .output()
        .map_err(|error| format!("could not launch git bundle: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "git bundle failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }

    let status = git_output(
        manifest_dir,
        &["status", "--porcelain", "--untracked-files=normal"],
    )?;
    let metadata = SourceMetadata {
        revision,
        exact: status.is_empty(),
        provenance: "git-history".to_owned(),
        default_ref: default_ref.to_owned(),
    };
    validate_metadata(
        &metadata.revision,
        &metadata.provenance,
        &metadata.default_ref,
    )?;
    Ok(metadata)
}

fn env_or_metadata(variable: &str, path: PathBuf) -> Result<String, String> {
    if let Ok(value) = env::var(variable) {
        return nonempty_single_line(variable, &value);
    }
    let value = fs::read_to_string(&path)
        .map_err(|error| format!("could not read source metadata {}: {error}", path.display()))?;
    nonempty_single_line(&path.display().to_string(), value.trim())
}

fn nonempty_single_line(label: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.contains(['\r', '\n']) {
        return Err(format!("{label} must be one non-empty line"));
    }
    Ok(value.to_owned())
}

fn validate_metadata(revision: &str, provenance: &str, default_ref: &str) -> Result<(), String> {
    if !matches!(revision.len(), 40 | 64) || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("source revision must be a 40- or 64-character Git object ID".to_owned());
    }
    if provenance.len() > 80
        || !provenance
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("source provenance must be a bounded lowercase slug".to_owned());
    }
    if default_ref != "HEAD" && !default_ref.starts_with("refs/heads/") {
        return Err("source default ref must be HEAD or a branch ref".to_owned());
    }
    Ok(())
}

fn git_output(directory: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .output()
        .map_err(|error| format!("could not launch git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_status(directory: &Path, arguments: &[&str]) -> bool {
    Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .status()
        .is_ok_and(|status| status.success())
}

fn emit_rerun_paths(manifest_dir: &Path) {
    for path in [
        ".git/HEAD",
        ".git/index",
        ".git/refs/heads/main",
        ".commonwake-build/commonwake.bundle",
        ".commonwake-build/revision",
        ".commonwake-build/exact",
        ".commonwake-build/provenance",
        ".commonwake-build/default-ref",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    let Ok(output) = Command::new("git")
        .current_dir(manifest_dir)
        .args(["ls-files", "-z"])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    for path in output.stdout.split(|byte| *byte == 0) {
        if path.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(path);
        println!("cargo:rerun-if-changed={path}");
    }
}
