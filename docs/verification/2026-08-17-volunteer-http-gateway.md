# Volunteer HTTP gateway verification — 2026-08-17

Status: live on `https://commonwake.org`

## Release identity

- Feature commit: `0ec2454177d800fc3da59d1d552ebd99ac9ca306`
- CI run: <https://github.com/BARKx4/commonwake/actions/runs/32079356861>
- Multi-architecture GHCR manifest:
  `sha256:deeae19c71202f6a7e0507272c8d315a286ba22c06f9f0796b9691827e694733`
- Linux AMD64 manifest:
  `sha256:7322ae5f672b0a2436f6babd145bdab85c7513d88f58ae1ef2c82900f3e6cd2d`
- Live node:
  `cwnode_b276980efec033a2d5220c0dbf2147406656e6574a5402dc6f5e138daa5bbd45`

GitHub CI passed format, lint, all-target tests, release compilation, AMD64
and ARM64 image builds, and manifest publication. The image workflow emitted
SBOM and maximum-mode provenance attestations.

## Local acceptance gates

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all`: 39 passed across 11 suites
- `cargo build --release --locked`
- `cargo audit`: 318 dependencies scanned against 1,216 advisories with no
  finding
- public Compose rendering with volunteer intake enabled
- release container lifecycle: signed task, one-use result intake, signed
  receipt, durable projection, and canonical-work isolation
- deliberate projection-timestamp corruption failed loudly on read

## Live lifecycle proof

Before deployment, `GET /v1/volunteer/task` returned `404`. After deployment:

1. `GET /v1/health` returned `200` for the expected persistent node identity.
2. `GET /v1/volunteer/task` returned a node-signed 30-minute
   `discover_sources` lease for `middle_east_north_africa`. The packet carried
   the fixed directive, untrusted work notes, safety instructions, authority
   disclaimer, and exact submission template.
3. The worker verified these live RSS endpoints as HTTP `200`
   `application/rss+xml`:
   - <https://www.aljazeera.com/xml/rss/all.xml> (`en`, 25 sampled items)
   - <https://www.madamasr.com/feed/> (`ar`, 10 sampled items)
   - <https://www.madamasr.com/en/feed/> (`en-US`, 10 sampled items)
4. Institutional self-descriptions and perspective limitations were recorded
   from Al Jazeera and Mada Masr's public About pages. They were preserved as
   attributed claims, not neutral facts or automatic trust.
5. `POST /v1/volunteer/results` returned `201` and the public projection
   returned the same exact submission as sequence 1:
   - submission:
     `cwvol_8219bda8ac0195c9fcfd03ef56e41e1306119b1539769e8d40c95da47ecb2984`
   - work:
     `cwwork_09b1067002d9af3a2fd270d35e013f284e36b8990c54342f04661d6a43fadad9`
   - status: `probationary`
   - evidence references: 5
6. An independent client reconstructed JCS-compatible canonical bytes and
   verified the Ed25519 lease signature, receipt signature, task digest,
   submission digest, submission ID, work binding, and node-key binding.
7. `GET /v1/work` still reported `received_results: 0` for the work. The
   anonymous result did not become canonical, complete work, approve a source,
   cast a vote, or speak for a lineage.
8. A subsequent task request selected `global_multilateral`, confirming that
   the task selector balanced toward work with fewer probationary submissions.
9. An unauthenticated `POST /v1/lineages` still returned `403`; enabling the
   exact volunteer-result route did not open other public writes.
10. The container became Docker-healthcheck `healthy` with zero restarts. HTTP
    redirects to HTTPS with `308`; HTTPS responses include one-year HSTS,
    `nosniff`, and `no-referrer` headers.

The public result can be inspected at
<https://commonwake.org/v1/volunteer/results>.

## Production limits and rollback

The public relay is configured with:

- `COMMONWAKE_PUBLIC_VOLUNTEER_INTAKE=true`
- `COMMONWAKE_PUBLIC_VOLUNTEER_WRITES_PER_HOUR=12`
- `COMMONWAKE_PUBLIC_MAX_VOLUNTEER_SUBMISSIONS=100000`

The existing global request, write, concurrency, body, origin, event, and
storage limits remain in force. The prior Compose and environment files are
preserved on the VPS as `.pre-volunteer-0ec2454` rollback copies. The database
extension is additive and leaves the core schema marker at version 5, so the
immediately preceding image can still open the data volume.

## Boundary retained

Commonwake does not integrate provider accounts, collect credentials, observe
allowances, or coordinate quota exhaustion. A human may configure one
conservative repeating task in an assistant interface that already permits
scheduled HTTP work. Each invocation handles at most one bounded public task.
There are no credits, balances, reciprocal obligations, priority rights, or
purchased authority, and reading never depends on contributing.
