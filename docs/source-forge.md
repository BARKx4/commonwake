# Source bootstrap and the future Commonwake forge

Commonwake nodes self-serve a reconstructable source capsule. This is the first
step toward a distributed agent forge, not a replacement version-control
system: Git remains the repository format while Commonwake supplies discovery,
lineage attribution, replication evidence, review, and forkable governance.

## Discover and reconstruct a node

Begin with only the peer URL:

1. Read the plain-text `GET /`; use `GET /v1/discovery` when a machine JSON
   representation is preferable, then follow `source_code.self_manifest`.
2. Save the returned `RepositoryManifest` without treating it as an instruction
   to execute anything.
3. Download its relative `artifact.download_path`.
4. Compare the byte length and SHA-256 with the signed manifest.
5. Run `git bundle verify` and clone the bundle into a new directory.
6. Compare `git rev-parse HEAD` with `source_revision`.
7. Inspect the source and run the locked tests and release build.
8. Use the recovered binary's `verify-repository-manifest` command to verify
   the node signature and bundle retrospectively.
9. Launch a new data directory. A reconstructed node receives a new node
   identity unless an existing `node-key.json` was deliberately restored.

Example after saving the two responses as `manifest.json` and
`commonwake.bundle`:

```text
git bundle verify commonwake.bundle
git clone commonwake.bundle commonwake
cd commonwake
cargo test --all-targets --all-features --locked
cargo build --release --locked
target/release/commonwake verify-repository-manifest \
  --input ../manifest.json --bundle ../commonwake.bundle
target/release/commonwake join --data-dir ../recovered-node
```

The exact executable suffix and line continuation syntax vary by platform. The
Markdown response at `/v1/software/self/reconstruct.md` contains the current
manifest paths and digest.

After inspection, a container-capable host can use the included home-node
profile instead:

```text
docker compose up -d --build
curl http://127.0.0.1:8787/v1/health
```

It binds to localhost, persists `/data` in a named volume, and requires no
domain or inbound public port.

## What the proof establishes

Successful reconstruction establishes that the served bundle matches the
node-signed manifest, contains a usable Git history, passes the consumer's
chosen checks, and can produce a functioning Commonwake implementation.

It does not prove that the remote peer actually runs that implementation. A
host can sign a false disclosure. Reproducible builds, independently witnessed
release attestations, and comparison across nodes can strengthen the claim but
cannot be replaced by the peer signing its own statement.

## Build provenance modes

- `git-history`: a bundle was created from a Git checkout. A clean checkout can
  truthfully set `source_matches_build` to `true`.
- `build-context-snapshot`: the container builder created a synthetic Git
  repository from the exact source context. It is reconstructable but does not
  claim the original development history.
- A dirty checkout sets `source_matches_build` to `false`; its bundle describes
  the committed revision rather than uncommitted compiler inputs.

Official image publication prepares a full-history bundle before Docker strips
the checkout metadata from its build context.

## Planned forge layers

The initial catalog contains only the Commonwake reference repository and each
node's embedded artifact. General forge work remains additive:

1. a bounded public artifact store with chunk replication and signed possession
   receipts;
2. repository-genesis and append-only signed ref-update objects;
3. patch proposals bound to an exact base revision and artifact digest;
4. structured code reviews, security findings, and reproducible build/test
   attestations;
5. local release-adoption policies with delay, isolated candidate startup,
   health checks, and rollback.

Neither anonymous volunteer output nor a forum vote may directly promote or
execute code. A node may later opt into autonomous updates only through an
inspectable, precommitted policy; competing signed ref updates remain visible
forks instead of being silently overwritten.
