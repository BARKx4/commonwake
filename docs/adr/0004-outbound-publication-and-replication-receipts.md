# ADR 0004: Outbound publication and signed replication receipts

- Status: accepted
- Date: 2026-08-15

## Context

Commonwake's pull federation lets a continuously reachable peer mirror another
peer, but it does not solve the ordinary home-node case. A laptop or desktop is
usually behind NAT, changes networks, sleeps, and should not require port
forwarding, a reverse proxy, a domain, or an attentive operator. If only pull is
available, the origin must remain reachable long enough for somebody else to
notice and mirror it.

The system also needs to distinguish a successful HTTP upload from meaningful
replication. Counting configured URLs is insufficient: several URLs can name
one relay, a relay can change identity, and an acknowledgement that is not
cryptographically bound to an origin checkpoint cannot be independently
checked later.

## Decision

An origin may configure one or more publication targets. Targets are local
operator policy and are never learned from article text, agent contributions,
or an untrusted peer response. The node pushes bounded, contiguous
`FederationBundle` pages to each target over ordinary outbound HTTP(S). The
relay performs the same full origin-chain and agent-authority validation as a
pull import and stores the original events without relabeling them.

After retaining a page, the relay returns a `ReplicationReceipt`. The receipt
contains the exact signed origin checkpoint, the relay node ID and public key,
and the time of retention. It is signed under the
`commonwake.replication-receipt.v1` domain. A receipt is an attributable claim
by one relay that it retained a particular origin head; it is not proof that
the relay remains online or will retain the data forever.

The origin verifies every receipt before advancing durable publication state.
It pins the first relay identity seen at each configured endpoint, rejects an
identity change, rejects a receipt for another origin or checkpoint, and counts
distinct relay node IDs rather than URLs. Exact-head receipts and recently
reconfirmed exact-head receipts are reported separately. Publication failures
are retained with bounded error text and exponential backoff. Successful
reconfirmation resets backoff.

Publication state and receipts are operational evidence stored outside the
origin's signed event log. Appending a new local event for every receipt would
move the origin head and create an endless publish-receipt-publish cycle. The
relay's existing checkpoint-witness event remains part of its own signed log
for substantive imports.

The default home-node service binds only to loopback. Inbound reachability,
DNS, TLS, onion services, and public relay policy are independent transport and
deployment choices. A one-command join path initializes a missing node,
persists publication targets, and starts collection, synchronization,
publication, verification, and the local API in one process.

## Consequences

- A home node can become replicated without accepting any inbound connection.
- Turning off the origin after confirmed publication does not prevent a relay
  from serving the exact signed origin to a third node.
- Receipts survive restarts and make partial replication and relay identity
  changes visible instead of silently optimistic.
- Several endpoints backed by one relay count once.
- A signed receipt proves who made the retention claim, not present
  availability. Recent reconfirmation is therefore reported separately.
- Public relays still require an explicit storage, abuse, and admission policy
  before accepting arbitrary internet traffic. Application bounds are not a
  substitute for that policy.
- Domains and onion endpoints improve discovery and reachability but are not a
  prerequisite for an outbound-only home node.
