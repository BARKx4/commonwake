use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::{
    CONSTITUTION_VERSION, PROTOCOL_VERSION,
    crypto::{encode, generate_signing_key, prefixed_id, sha256_hex, signing_key_from_b64},
    db::Database,
    error::{CommonwakeError, Result},
    model::{Checkpoint, PolicyView},
};

const NODE_KEY_FILE: &str = "node-key.json";
const DATABASE_FILE: &str = "commonwake.db";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeKeyFile {
    protocol: String,
    node_id: String,
    created_at: DateTime<Utc>,
    public_key: String,
    secret_key: String,
}

pub struct NodeIdentity {
    node_id: String,
    public_key: String,
    signing_key: SigningKey,
    created_at: DateTime<Utc>,
}

impl NodeIdentity {
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    pub fn sign_hash(&self, hash: &[u8; 32]) -> String {
        use ed25519_dalek::Signer;
        encode(self.signing_key.sign(hash).to_bytes())
    }
}

#[derive(Clone)]
pub struct CommonwakeNode {
    pub db: Arc<Database>,
    pub identity: Arc<NodeIdentity>,
    pub data_dir: Arc<PathBuf>,
    pub policy: PolicyView,
}

impl CommonwakeNode {
    pub fn initialize(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        fs::create_dir_all(data_dir)?;
        let key_path = data_dir.join(NODE_KEY_FILE);
        if key_path.exists() {
            return Err(CommonwakeError::Conflict(format!(
                "node already initialized at {}",
                data_dir.display()
            )));
        }

        let signing_key = generate_signing_key()?;
        let public_key = encode(signing_key.verifying_key().to_bytes());
        let node_id = prefixed_id("cwnode_", &signing_key.verifying_key().to_bytes());
        let key_file = NodeKeyFile {
            protocol: PROTOCOL_VERSION.into(),
            node_id,
            created_at: Utc::now(),
            public_key,
            secret_key: encode(signing_key.to_bytes()),
        };
        write_secret_json(&key_path, &key_file)?;
        Self::open(data_dir)
    }

    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let key_path = data_dir.join(NODE_KEY_FILE);
        if !key_path.exists() {
            return Err(CommonwakeError::NotFound(format!(
                "no node identity at {}; run `commonwake init` first",
                data_dir.display()
            )));
        }

        let key_file: NodeKeyFile = serde_json::from_slice(&fs::read(&key_path)?)?;
        if key_file.protocol != PROTOCOL_VERSION {
            return Err(CommonwakeError::Validation(format!(
                "node key uses unsupported protocol {}",
                key_file.protocol
            )));
        }
        let signing_key = signing_key_from_b64(&key_file.secret_key)?;
        let derived_public = encode(signing_key.verifying_key().to_bytes());
        let derived_id = prefixed_id("cwnode_", &signing_key.verifying_key().to_bytes());
        if derived_public != key_file.public_key || derived_id != key_file.node_id {
            return Err(CommonwakeError::Unauthorized(
                "node key file does not match its public identity".into(),
            ));
        }

        let identity = Arc::new(NodeIdentity {
            node_id: key_file.node_id,
            public_key: key_file.public_key,
            signing_key,
            created_at: key_file.created_at,
        });
        let db = Arc::new(Database::open(data_dir.join(DATABASE_FILE))?);
        db.bind_node(&identity)?;

        let policy = PolicyView {
            constitution_version: CONSTITUTION_VERSION.into(),
            digest: sha256_hex(include_bytes!("../docs/constitution.md")),
        };

        Ok(Self {
            db,
            identity,
            data_dir: Arc::new(data_dir),
            policy,
        })
    }

    /// Open an existing node or initialize a genuinely empty location.
    ///
    /// A database without its node key is never silently rebound to a new
    /// identity; that is a recovery problem and must fail loudly.
    pub fn open_or_initialize(data_dir: impl AsRef<Path>) -> Result<(Self, bool)> {
        let data_dir = data_dir.as_ref();
        if data_dir.join(NODE_KEY_FILE).exists() {
            return Ok((Self::open(data_dir)?, false));
        }
        if data_dir.join(DATABASE_FILE).exists() {
            return Err(CommonwakeError::Conflict(format!(
                "database exists at {} but its node key is missing; restore node-key.json instead of creating a new identity",
                data_dir.display()
            )));
        }
        Ok((Self::initialize(data_dir)?, true))
    }

    pub fn checkpoint(&self) -> Result<Checkpoint> {
        let (cursor, _) = self.db.current_head()?;
        self.checkpoint_at(cursor)
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.identity.created_at
    }
}

fn write_secret_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
