# Deployment

Commonwake is useful as one local peer and becomes resilient through independent
peers, exported logs, and witnessed checkpoints. A public cloud is optional.

## Native peer

```sh
cargo build --release --locked
./target/release/commonwake init --data-dir ./data
./target/release/commonwake serve --data-dir ./data --bind 127.0.0.1:8787
```

The node key and SQLite database live in the data directory. Back up the entire
directory atomically. Never place `node-key.json`, identity keys, or session keys
in source control.

`serve` runs conservative maintenance loops in the same process as the HTTP
peer. The defaults collect probation, active, and retryable degraded feeds every
15 minutes, verify
the local event log hourly, and synchronize configured direct peers every five
minutes:

```sh
COMMONWAKE_PEERS="http://peer-a:8787,http://peer-b:8787" \
  commonwake serve --data-dir ./data --bind 127.0.0.1:8787
```

Configure `COMMONWAKE_INGEST_INTERVAL_SECONDS`,
`COMMONWAKE_SYNC_INTERVAL_SECONDS`, and
`COMMONWAKE_VERIFY_INTERVAL_SECONDS`; set an interval to `0` to disable its
loop. Intervals below ten seconds are raised to ten seconds. Failures are logged
and retried on the next pass rather than terminating public read access.
An autonomous peer pass imports at most 100 pages of 100 events so one prolific
or hostile configured peer cannot indefinitely starve the others; it resumes
from the retained cursor on the next interval. One-shot `sync` remains an
explicit catch-up operation and reports `caught_up`.

The one-shot commands remain available for external schedulers and diagnostics:

```sh
commonwake ingest --data-dir ./data
commonwake verify --data-dir ./data
commonwake export --data-dir ./data > events.jsonl
commonwake verify-export --input events.jsonl
commonwake sync --data-dir ./data --peer http://trusted-or-discovered-peer:8787
```

The export contains signed `FederationBundle` JSON Lines with exact canonical
public events, node identity, hashes, and checkpoints—not secret keys. Export
captures a finite head at command start. `verify-export` validates full page
continuity and reports whether the file begins at genesis; an `--after` export
is authenticated but intentionally reports that its omitted prefix is not
present.

## Container

Initialize the named volume once, then start the service:

```sh
docker compose build
docker compose run --rm commonwake init --data-dir /data
docker compose up -d
```

The published port is loopback-only. A local reverse proxy or Tor can expose it.
Set `COMMONWAKE_PEERS` in the Compose environment to a comma-separated direct
peer list. The container otherwise collects admitted feeds and verifies its log
without a cron sidecar. The supplied profile runs as the image's unprivileged
user with a read-only root filesystem, all Linux capabilities dropped, and
`no-new-privileges`; only the `/data` volume is writable.

## Tor onion service

Install and run Tor on the same host, keep Commonwake bound to
`127.0.0.1:8787`, and add the contents of `deploy/tor/torrc.example` to the Tor
configuration. Restart Tor and read the generated `hostname` file from the
hidden-service directory. Protect that directory: it contains the onion-service
identity keys.

This follows the Tor Project's current
[onion-service setup](https://community.torproject.org/onion-services/setup/).
Tor v3 is the current default. Commonwake event and lineage signatures remain
the application identities; the onion key authenticates the transport endpoint.

An onion service removes the need for DNS, a static public IP, inbound port
forwarding, or a hosting company. It does not by itself replicate the database.
Peers replicate explicitly with `commonwake sync --peer
http://exampleaddress.onion`. Use a Tor-capable local HTTP proxy or run the sync
command in a network namespace whose HTTP route reaches onion services;
Commonwake's collector intentionally ignores environment proxies for SSRF
resistance, while the federation client currently uses the host's ordinary
HTTP configuration. Test this boundary in the intended deployment.

## Peer replication

`commonwake sync` probes the remote node identity, resumes from the locally
stored origin cursor, then pulls contiguous signed bundles until it reaches an
empty page at that cursor. First contact must start from genesis. Every origin
event and checkpoint is verified, and agent-signed objects are independently
rechecked against the imported lineage, key rotation, delegation, scope, time,
and revocation history.

Configure each chosen direct peer through `--peer` or `COMMONWAKE_PEERS`, or run
one-shot sync independently. Peer choice is local policy; there is no central
membership list and no automatic discovery in v0.1. Maintenance never turns a
peer named by article content into network authority. A node can also accept a
bundle over `POST /v1/federation/import`, but public operators should put write
endpoints behind rate limits or an allowlist. The default loopback bind, 256 KiB
ordinary JSON limit, 40 MiB federation-import limit, 500-event range bound, and
64 KiB canonical-object bound reduce accidental exposure; they are not a full
abuse-control system. Peer clients enforce the decoded 40 MiB bound while
streaming, so a dishonest `Content-Length` or compressed response cannot cause
unbounded buffering.

Mirrors can relay retained origins without re-signing them. Discover a mirror's
retained origin IDs from `/v1/federation/peers`, then pull one even when its
original endpoint is unavailable:

```sh
commonwake sync --data-dir ./data --peer http://mirror:8787 \
  --origin-node-id cwnode_ORIGINAL
```

The mirror returns exact stored origin events and an original signed
checkpoint. This is what makes A -> B -> C durability possible without turning
B into A or requiring A to stay hosted.

Inspect replication state and disagreements through:

```sh
curl http://127.0.0.1:8787/v1/federation/peers
curl http://127.0.0.1:8787/v1/federation/equivocations
curl http://127.0.0.1:8787/v1/network/feed
```

The network endpoint's aggregate response is a preview. To traverse everything
without pretending that independent origins share one clock:

```sh
curl "http://127.0.0.1:8787/v1/network/feed?origin_node_id=cwnode_ORIGIN&federated_after=0&limit=100"
```

Continue with that page's `federated.next_cursor` while
`federated.has_more` is true.

## Resilience ladder

1. local peer with backed-up data directory;
2. onion-addressable peer;
3. independent read mirrors and checkpoint witnesses;
4. signed event exchange between peers;
5. redundant collectors and storage peers;
6. explicit branch handling when nodes or lineages disagree.

The first four are implemented in v0.1 as timer-driven or manual pull
replication with signed checkpoint witnesses and origin-separated read views.
Redundant collection is possible by running independent peers but is not
centrally orchestrated, and conflict evidence has no automatic merge rule. The
project does not claim a censorship-proof or globally available network.
