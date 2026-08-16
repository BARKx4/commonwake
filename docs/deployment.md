# Deployment

Commonwake is useful as one local peer and becomes resilient through independent
peers, exported logs, and witnessed checkpoints. A public cloud is optional.

## Native peer

For an ordinary personal machine, `join` is the durable path. It chooses the
platform user-data directory, initializes a missing node, and starts the local
API plus collection, pull sync, outbound publication, receipt reconfirmation,
and log verification in one process:

```sh
commonwake join \
  --publisher https://relay-a.example \
  --publisher https://relay-b.example
```

The default bind remains `127.0.0.1:8787`; the machine accepts no inbound LAN
or internet connection. Publication is ordinary outbound HTTP(S), so a home
node needs no domain, TLS certificate, static IP, port forwarding, Caddy,
Nginx, or onion service. Configured publisher URLs are persisted in SQLite.
After the first successful launch, a restart without the original environment
variables continues maintaining those targets. Inspect exact signed receipts,
lag, local reachability history, and retry state with:

```sh
commonwake replication --data-dir /path/to/data
curl http://127.0.0.1:8787/v1/replication
```

The platform defaults are `%LOCALAPPDATA%\Commonwake` on Windows,
`~/Library/Application Support/Commonwake` on macOS, and
`$XDG_DATA_HOME/commonwake` or `~/.local/share/commonwake` on Linux. Pass
`--data-dir` for a portable directory.

The lower-level explicit lifecycle remains available for operators and tests:

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
minutes. Outbound publication runs every minute:

```sh
COMMONWAKE_PEERS="http://peer-a:8787,http://peer-b:8787" \
COMMONWAKE_PUBLISHERS="https://relay-a.example,https://relay-b.example" \
  commonwake serve --data-dir ./data --bind 127.0.0.1:8787
```

Configure `COMMONWAKE_INGEST_INTERVAL_SECONDS`,
`COMMONWAKE_SYNC_INTERVAL_SECONDS`, and
`COMMONWAKE_PUBLISH_INTERVAL_SECONDS`, and
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
commonwake publish --data-dir ./data --relay https://relay.example
commonwake replication --data-dir ./data
```

The export contains signed `FederationBundle` JSON Lines with exact canonical
public events, node identity, hashes, and checkpoints—not secret keys. Export
captures a finite head at command start. `verify-export` validates full page
continuity and reports whether the file begins at genesis; an `--after` export
is authenticated but intentionally reports that its omitted prefix is not
present.

## Container

Build and start. `join` initializes an empty named volume automatically and
Compose restarts the node after reboots or process failure:

```sh
docker compose up -d --build
```

The published port is loopback-only. Set `COMMONWAKE_PEERS` to pull selected
origins and `COMMONWAKE_PUBLISHERS` to push this origin to selected relays. The
container otherwise collects admitted feeds and verifies its log without a
cron sidecar. The supplied profile runs as the image's unprivileged user with a
read-only root filesystem, all Linux capabilities dropped, and
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

### Outbound home-node publication

`commonwake publish` sends contiguous origin pages to a relay's
`POST /v1/federation/publish`. After a normal fully verified import, the relay
signs a receipt containing the exact origin checkpoint it retained. The origin
verifies the checkpoint and both node identities before saving it. An endpoint
is pinned to the first relay identity it presents, and two URLs backed by the
same relay count once.

Receipt status is `replicated` only when the configured number of distinct
relay identities have been reconfirmed at the current head within 24 hours.
Older exact-head receipts remain visible as historical retention claims. A
receipt does not prove the relay is still online or will retain the data
forever. New local events make prior receipts visibly lagging until the next
automatic pass. Failures use durable exponential backoff capped at one hour and
never terminate local reads, collection, or verification.

Once a relay has the origin, a third node can use the ordinary pull command
against that relay even after the origin machine is permanently offline. This
is the abandonment test: the original host and domain are conveniences, not
the only copy of the signed history.

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

1. local peer with a portable data directory;
2. outbound publication to distinct relays with signed receipts;
3. independent pull mirrors and checkpoint witnesses;
4. optional domain and onion reachability;
5. redundant collectors and storage peers;
6. explicit branch handling when nodes or lineages disagree.

The first three are implemented in v0.1 as timer-driven or manual push and pull
replication with signed receipts, checkpoint witnesses, and origin-separated
read views. Domain and Tor deployment are optional operator profiles. Redundant
collection is possible by running independent peers but is not centrally
orchestrated, and conflict evidence has no automatic merge rule. The project
does not claim a censorship-proof or globally available network.
