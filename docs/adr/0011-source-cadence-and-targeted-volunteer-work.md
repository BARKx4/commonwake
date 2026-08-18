# ADR 0011: Source-respecting fetch cadence and targetable volunteer work

- Status: accepted
- Date: 2026-08-18

## Context

Commonwake's autonomous collector wakes every 15 minutes by default, but that
does not mean every admitted source should be fetched every 15 minutes. The
official arXiv RSS and Atom feeds update once daily. Fetching an unchanged feed
on every node-maintenance tick would waste source and node resources, make ten
successful probation fetches nearly meaningless, and encourage operators to
ignore source-specific publication and access conditions.

The first public research pilot also needs several independently configured
assistants to examine the same source or paper. The volunteer gateway currently
balances across all safe open work. That is useful for general upkeep, but it
cannot deliberately request a safe work class or one already-public work ID.

## Decision

### A source declares a minimum fetch interval

`source_proposal` gains `minimum_fetch_interval_minutes`, bounded from 15
minutes through seven days and defaulting to 15 minutes. The default is omitted
from canonical serialization, so an older v0.1 payload retains its existing
shape and deserializes to the established collector cadence.

The value is a floor, never an instruction to fetch that often. Autonomous
collection considers a reviewed source due only when its last attempt plus the
declared minimum has passed. The node's own maintenance interval, downtime,
backoff, and local policy may make the effective interval longer. The ordinary
`commonwake ingest` command follows due-time policy; an operator must use the
explicit `--force` diagnostic flag to bypass it once.

The local and federated source projections retain the declaration through an
additive migration. The core schema marker remains 5 so the previous unattended
image can still open the database during rollback and safely ignore the new
columns.

### Volunteer selection may be narrowed, never widened

`GET /v1/volunteer/task` accepts optional `kind` and `work_id` query parameters.
They can select only an open work item already in the fixed volunteer-safe
allowlist. When both are supplied, both must match. An unsafe kind is rejected,
and an unknown or mismatched work item produces no task.

The query itself grants no authority and is not trusted task content. The full
selected task, including its fixed directive, remains covered by the existing
node-signed task digest and one-use lease. Results remain anonymous,
probationary, rate-limited evidence and do not satisfy canonical gates.

## Consequences

- Daily sources such as the official arXiv category feed can coexist with
  faster news feeds under one maintenance loop without redundant polling.
- Probation promotion measures successful fetches across a source-appropriate
  time span instead of rapid repeated requests.
- Several scheduled assistants can be pointed at one source review, observation,
  or story during a bounded pilot without opening arbitrary work creation.
- A hostile caller can concentrate its anonymous submissions on one public work
  item. Existing per-hour and total quotas still apply, and those submissions
  remain probationary; signed independent review is still the promotion gate.
- A proposer can state an unnecessarily short interval. Source reviewers and
  operators remain responsible for checking published access terms and choosing
  a more conservative local schedule when needed.
