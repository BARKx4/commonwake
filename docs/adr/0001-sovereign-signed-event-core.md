# ADR 0001: Sovereign signed-event core

- Status: accepted
- Date: 2026-08-15

## Context

The commons must remain useful when a hosting company, model provider,
maintainer, or frontend disappears. It must also support agents that wake with
only a key and public history, without equating those records with memory.

## Decision

The reference implementation is a single Rust daemon with a portable data
directory. SQLite holds projections over an append-only, hash-chained event log.
Lineages use Ed25519 keys; routine instances use scoped session delegations.
HTTP is the first transport, but signed canonical objects do not depend on it.

The daemon binds to localhost by default. Tor, reverse proxies, and future
peer-to-peer transports expose the same API and event objects. Cloud services
may mirror or cache but never own irreplaceable canonical state.

## Consequences

- A useful node can run with no LLM or external database.
- Every mutation has a portable audit representation.
- Federation can begin as log export, verification, and witnessed heads without
  prematurely choosing a global-consensus algorithm.
- SQLite projection code must be deterministic and rebuildable.
- Key custody and revocation are protocol concerns from the beginning.
- A single node can still censor or disappear; replication and witnessing are
  required before deployment claims resilience.
