# ADR 0005: Native public edge and ACME

- Status: accepted
- Date: 2026-08-17

## Context

An ordinary Commonwake node should need only outbound HTTP. A voluntary public
relay has a different job: it must remain safely readable on the internet after
its human operator loses interest. Requiring Caddy, Nginx, hand-managed
certificates, a second policy configuration, and an unbounded application
listener turns the relay into a conventional server project and creates
several places for admission rules to drift.

The binary still needs an unrestricted local API for administration. Exposing
that same router publicly would allow anyone to allocate durable state, import
arbitrary origins, and consume disk or verification time.

## Decision

Native public service is opt-in through one TLS DNS name. When enabled, the
process runs three listeners:

1. the existing unrestricted HTTP router, which must bind to loopback;
2. an HTTP listener serving only ACME HTTP-01 and a redirect to the configured
   fixed HTTPS name;
3. a native rustls HTTPS listener using a distinct bounded public router.

Certificate and ACME account state lives inside the portable node data
directory. Let's Encrypt staging is the default; production issuance requires
an explicit setting. Redirects never trust the request `Host` header.

Public reads need no account or payment. Ordinary writes require a configured
bearer secret. `POST /v1/federation/publish` may alternatively admit complete
origin-node IDs from local policy, after which all existing signature, chain,
authority, and body validation still applies. With neither mechanism the
public endpoint is read-only.

The public edge places global bounds on requests per second, writes per minute,
concurrency, concurrent large federation bodies, total data-directory
headroom, distinct retained origins, and the maximum cursor retained for any
one origin. Public federation admission is serialized so concurrent new
origins cannot race past the origin-count policy. It also retains the existing
route-body, event-count, and canonical-object bounds. Resource exhaustion
pauses writes while leaving reads available. Admission settings come only from
operator arguments or environment, never article text, agent contributions,
or peer responses.

The reference public container maps unprivileged internal ports to host ports
80 and 443, keeps the unrestricted listener inside the container loopback,
drops all capabilities, and uses a read-only root filesystem. CI publishes
multi-architecture images only after the full validation job. The optional
host timer tags the current image, tries the new CI-tested image, and attempts
the prior image if local health fails. The unattended release channel must keep
data migrations compatible with the immediately preceding image because image
rollback never rewinds node state.

## Consequences

- A public relay needs one binary and one data volume, not a reverse proxy or a
  cron sidecar.
- Transport configuration cannot silently bypass application admission by
  pointing at the unrestricted router.
- A fresh public relay is useful for reads but cannot be filled by anonymous
  writes.
- An admitted origin is attributable but not necessarily honest or
  independent; cryptographic validity remains distinct from truth.
- Rate windows are process-local and deliberately simple. Multi-instance
  deployment needs an upstream or shared limiter and is outside this profile.
- Storage is bounded by metadata accounting, not a reserved filesystem quota.
  Operators should still use host disk monitoring and independent backups.
- Native TLS does not supply DDoS absorption, global anonymity, automatic peer
  discovery, or permanent availability. Replication remains the durability
  mechanism.
