# Four-node abandonment lab

This profile proves the property that matters more than a successful upload:
an outbound-only origin can disappear and a previously uninvolved reader can
still recover and verify its exact history from a relay.

Start the origin, two distinct relays, and a blank reader:

```sh
docker compose -f deploy/lab/compose.yaml up -d --build
```

The host ports are origin `18787`, relays `18788` and `18789`, and reader
`18790`. All application state is in named volumes. The origin automatically
publishes outbound to both relays; none of these containers needs a reverse
proxy, domain, or inbound host port for inter-node traffic.

Create one attributable origin event, force an immediate publication pass, and
inspect the two signed receipts:

```sh
docker compose -f deploy/lab/compose.yaml exec origin \
  commonwake identity create --display-name lab-author --out /data/lab-author.json
docker compose -f deploy/lab/compose.yaml exec origin \
  commonwake register --server http://127.0.0.1:8787 --identity /data/lab-author.json
docker compose -f deploy/lab/compose.yaml exec origin \
  commonwake publish --data-dir /data --relay http://relay-a:8787
docker compose -f deploy/lab/compose.yaml exec origin \
  commonwake publish --data-dir /data --relay http://relay-b:8787
curl http://127.0.0.1:18787/v1/replication
```

Copy the origin node ID from that response. Stop—but do not remove—the origin,
then recover through relay A:

```sh
docker compose -f deploy/lab/compose.yaml stop origin
docker compose -f deploy/lab/compose.yaml exec reader \
  commonwake sync --data-dir /data --peer http://relay-a:8787 \
  --origin-node-id cwnode_ORIGIN
curl http://127.0.0.1:18790/v1/federation/peers
```

The reader revalidates the origin's identity, event hash chain, checkpoint,
lineage signature, and authority history. Relay A never becomes the origin.
Restart the stopped origin with `docker compose -f deploy/lab/compose.yaml start
origin`. This lab deliberately provides no command that deletes its volumes.
