# Topic commons and sealed-mail verification — 2026-08-17

This record covers the first proof of signed topic governance, evidence-linked
discussion, reversible dormancy, OpenPGP key announcements, and content-sealed
public-metadata mail.

## Static and test gates

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: clean.
- `cargo test --all-targets --all-features --locked`: 33 passed across nine
  executable test suites.
- `cargo build --release --locked`: succeeded.
- `cargo audit --quiet`: no reported advisories.
- `git diff --check`: clean.

The forum lifecycle test creates three independent lineages, proves that a
proposer cannot count as an independent voter, crosses the two-approval gate,
posts a threaded discussion with signed references to canonical Commonwake
objects, pages topics and posts, publishes a key, routes sealed content,
federates the complete state, exposes a cross-origin vote conflict, converges
the vote, revokes the key, and verifies both node logs.

The v1-upgrade test proves that the new tables and `topic_commons_schema = 1`
feature marker are installed while the core schema marker remains 5. The
extension is additive and idempotent so the immediately preceding unattended
image can still open the data directory during rollback and ignore the new
tables.

## Container gate

The exact local release image was:

```text
sha256:0ea3f6947f48ee0998805438bf94d067de456cea331ed6456837bff829109efc
```

It initialized and became healthy as UID `commonwake` with a read-only root
filesystem, all capabilities dropped, `no-new-privileges`, bounded CPU,
memory, and process count, and only an in-memory `/data` mount writable.
`/v1/health`, the topic index, and offline signed-log verification all
succeeded. The disposable container and its in-memory data were removed after
the check.

## Cryptographic boundary

The lifecycle certificate and message are armor-shaped transport fixtures, not
real OpenPGP cryptographic test vectors. The reference peer deliberately checks
signed Commonwake routing, bounds, armor shape, fingerprint shape, and key
lifecycle only. A current RFC 9580 client remains responsible for parsing the
certificate, deriving and comparing its full fingerprint, encrypting,
decrypting, and validating any inner signature.
