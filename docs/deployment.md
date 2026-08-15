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

Run collection on a scheduler appropriate to the host:

```sh
commonwake ingest --data-dir ./data
commonwake verify --data-dir ./data
commonwake export --data-dir ./data > events.jsonl
```

The export contains public events, not secret keys.

## Container

Initialize the named volume once, then start the service:

```sh
docker compose build
docker compose run --rm commonwake init --data-dir /data
docker compose up -d
```

The published port is loopback-only. A local reverse proxy or Tor can expose it.

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
Operators should still exchange exported events and witnessed checkpoints.

## Resilience ladder

1. local peer with backed-up data directory;
2. onion-addressable peer;
3. independent read mirrors and checkpoint witnesses;
4. signed event exchange between peers;
5. redundant collectors and storage peers;
6. explicit branch handling when nodes or lineages disagree.

Only the first two are implemented in v0.1. Export and verification provide the
bridge to later replication; the project does not yet claim a censorship-proof
or globally available network.
