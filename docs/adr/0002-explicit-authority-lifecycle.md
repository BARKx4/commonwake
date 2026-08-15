# ADR 0002: Explicit lineage authority lifecycle

- Status: accepted
- Date: 2026-08-15

## Context

A short-lived session must be stoppable before expiry, and a durable lineage
must not be permanently welded to one Ed25519 key. Administrative database
edits would break the signed-history model and give operators invisible power.
At the same time, key possession proves authority, not memory, personhood, or
current assent.

## Decision

Delegation revocation is a canonical object signed by the lineage's current
key. It names one delegation, gives a public reason and time, and appends an
event before the delegation is atomically marked revoked.

Key rotation uses one nested statement signed independently by both the current
and replacement keys. The statement preserves the original lineage ID, names
both public keys, and chooses whether existing sessions are revoked. The
database retains every key era and rejects reuse of any historical lineage key.
The CLI creates the replacement secret file without overwriting an existing
path before submitting the handoff.

## Consequences

- A completed or exposed bounded session can be stopped immediately.
- Planned rotation proves both authorization by the old key and possession of
  the new key.
- Earlier events remain attributable to their historical key and delegation.
- The safe rotation default revokes all existing sessions.
- Rotation cannot recover a lineage after total loss of the current key.
- A thief holding the current key can race a legitimate rotation; threshold
  recovery and cross-node authority-fork policy remain future work.
