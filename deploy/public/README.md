# Public relay without a reverse proxy

This profile gives one Commonwake process three deliberately separate doors:

- private loopback HTTP on container port `8787` for health and administration;
- public port `80` only for ACME HTTP-01 and fixed-domain HTTPS redirects;
- public native TLS on port `443`, always behind Commonwake's bounded edge.

Copy `.env.example` to `.env`, set the DNS name, and leave ACME production
disabled for the first certificate attempt. Then run:

```sh
docker compose up -d --wait
docker compose logs --tail=100 commonwake
```

Once DNS resolves to this host and the logs show a successful staging
certificate, set `COMMONWAKE_ACME_PRODUCTION=true` and run the command again.
The default public edge is read-only. An empty bearer token and empty publisher
list do not admit writes. Ordinary writes require a bearer token of at least 32
non-whitespace bytes. Signed federation publication may instead be limited to
explicit complete `cwnode_...` origin identifiers.

The container runs without Linux capabilities, with a read-only root, bounded
memory and processes, bounded application admission, and a size-limited log.
Only the named `/data` volume is durable. ACME account and certificate state is
stored under `/data/acme`, so ordinary container replacement does not discard
it. Docker-published ports can bypass UFW; starting this public profile is the
act that exposes ports 80 and 443, regardless of a host-level UFW deny rule.

For a host intended to outlive operator attention, copy this directory to
`/opt/commonwake`, copy the two supplied units to `/etc/systemd/system`, and
enable the timer:

```sh
systemctl daemon-reload
systemctl enable --now commonwake-update.timer
```

The updater tags the running image before pulling. A replacement must pass the
container's local health check within two minutes or the updater attempts to
restore the previous image. It never rolls back or deletes node data, so every
release on this unattended channel must keep its data migration compatible
with the preceding image. It also never prunes older images. Image publication
is gated by formatting, lint, tests, and a release build in GitHub Actions.
