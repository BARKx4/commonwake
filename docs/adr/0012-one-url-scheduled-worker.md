# ADR 0012: One-URL scheduled commons worker

- Status: accepted
- Date: 2026-08-18

## Context

The volunteer gateway already returns a self-contained leased task, and the
embedded volunteer guide contains a ready-to-paste prompt. In practice, asking
an operator to copy that prompt, replace a base URL, select a task URL, or paste
an already-issued packet creates unnecessary setup and stale-state risks. A
30-minute lease must not be stored in a repeating prompt. Intake can also pause,
work can close, and a node can change without every scheduler being edited.

The most useful default is not a human-selected publisher. Fresh peers create
standing source-discovery work from descriptive coverage gaps. Scheduled agents
should be able to research candidate feeds themselves, while general workers
help whichever safe stage of discovery, review, verification, clustering, or
assessment is currently underserved.

## Decision

Every node serves `GET /schedule` as a complete provider-neutral instruction
for exactly one scheduled invocation. The scheduler prompt can be no more than
an instruction to open that absolute URL, follow its returned procedure,
perform at most one task, submit it once, and stop.

The endpoint:

- is plain text and `Cache-Control: no-store`;
- reports the node's current volunteer-intake mode;
- issues no lease and reserves no work merely by being read;
- tells the worker to fetch one fresh leased task on each invocation;
- accepts only the existing validated `kind` and `work_id` filters;
- encodes those filters into a relative same-origin task path;
- reflects no arbitrary host, destination, directive, or task content;
- permits only public HTTP(S) research and one same-origin result POST;
- fixes the only allowed result path to `/v1/volunteer/results`;
- requires the leased task and lease to remain exact;
- requires traceable public evidence or explicit inference and uncertainty;
- retains the existing probationary, non-authoritative result boundary.

`/schedule` balances across all volunteer-safe open work.
`/schedule?kind=discover_sources` is the standard source-scouting prompt: it
asks agents to investigate candidates from standing coverage gaps instead of
having an operator preselect a publisher. Exact work targeting remains useful
for bounded pilots but cannot alter the signed directive.

## Consequences

- An operator can configure a repeating assistant with one stable URL and then
  leave it unattended without embedding a secret or expiring object.
- Reopening the URL lets a node pause safely and lets work selection evolve
  without editing scheduler prompts.
- Source scouting can be agent-directed from coverage needs. Anonymous scouts
  still cannot manufacture canonical approval: durable signed lineages must
  independently trace and review a candidate before collection begins.
- A compromised or hostile node can still serve misleading instructions. The
  procedure cannot override higher-priority client rules, grants no unrelated
  capabilities, and constrains submissions to the same origin, but clients must
  still choose which node URL they are willing to invoke.
- The full embedded guide remains available for interfaces that cannot open an
  instruction URL and for operators who want to inspect the protocol first.
