# Abandonment-lab verification: 2026-08-15

## Build and static verification

- `cargo test --locked --all-targets`: 24 passed across 7 suites.
- `cargo clippy --locked --all-targets -- -D warnings`: no issues.
- `docker compose -f deploy/lab/compose.yaml config --quiet`: accepted.
- Container image:
  `sha256:386b2d48ab0302731c627f08add393fcf1d951965177dcdffaf0331e9ee0a330`.

## Live four-node result

The lab started one outbound-only origin, two distinct relays, and one blank
reader. The origin initialized itself, created one signed
`lineage_registered` event, and published cursor 1 to both relays.

- origin:
  `cwnode_9f361f1222ee591a91bd19110acbfb6c16e1ad52a464c975c6358b19230315c5`
- relay A:
  `cwnode_bc435852842aa1c775da2995230664bf760a1d589dc03165f0c8a143a8a421ea`
- relay B:
  `cwnode_1b76eac4494977bf5067ddad8e59d7f775fe0d5cc0ddfc5a1673f3bf226706ce`
- event:
  `cwevt_9faa80b62b5c9629c02829a2fd8eace2afa7b7668391f62d0eddd774825f936e`
- event hash:
  `e02439d4bc604e5fe448f872e3f5c1e1e5a5442404af174f0088bb48d0aaf61f`

The origin's replication endpoint reported `replicated`, cursor 1, two exact
current receipts, and two recently reconfirmed distinct relay identities.

The origin container was then stopped and its host API was confirmed
unreachable. The previously blank reader synchronized the named origin through
relay A, imported one event, created its own checkpoint-witness event, and
reported the original cursor and event hash without relabeling relay A as the
origin.

The origin was restarted from the same volume and returned with the same node
ID, cursor 1, both configured targets, and both receipts. The first container
stop exposed a missing Unix `SIGTERM` path (exit 137); after wiring `SIGTERM` to
Axum's graceful shutdown, all four containers stopped with exit code 0.

All lab containers are stopped. Their named volumes remain intact; no lab
volume was deleted.

The live lab used a small identity event so the evidence is easy to audit. The
integration suite separately carries reviewed source manifests, RSS
observations, assessments, regional perspective metadata, and wake orientation
through the same origin-preserving federation path.
