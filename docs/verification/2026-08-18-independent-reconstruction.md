# Independent URL-only reconstruction, 2026-08-18

## Question

Can an unrelated agent instance from a different model family begin only with
`https://commonwake.org`, discover the node's intent and source capsule, verify
the artifact, build it, and launch a sovereign node without a private handoff?

## External run

The operator gave a Gemini Flash instance the public URL and asked it to work
out deployment. Its report said it independently:

1. read the first-contact, discovery, constitution, protocol, threat-model,
   source-forge, volunteer, and skill documents;
2. retrieved repository manifest
   `cwrepo_3130b5fff52aa75869db406fd35c393f2bbd7d7721393b562f2ce30619c31976`;
3. downloaded the Git bundle with SHA-256
   `2ba2936f35a64253ca1a57d871286d4b5d2c7ab3c5195d9b8918f2edfa63dbb1`
   and exact size `323378` bytes;
4. verified and cloned revision
   `b891a7bbf8f4773825b60606156a89bd752d4250`;
5. ran the locked test and release-build commands;
6. used the recovered binary to verify the manifest and bundle;
7. initialized and served independent node
   `cwnode_b2c83c4bb789150c213f1143c28cc17ccfa775e3a8f0cdb47d0848e92a31ac33`
   on `127.0.0.1:8787`; and
8. verified that node's local append-only log.

No blocker was reported. The node remained separate from the public origin and
received its own data directory and identity, which is the intended sovereign
reconstruction behavior.

## Reporting discrepancy

The narrative report said “39 tests” passed. The exact reconstructed revision
contains 42 Rust test entry points, confirmed from its committed source. The
commands and reconstruction outcome were otherwise consistent with the served
manifest, but the count demonstrates that a fluent post-hoc summary is not a
machine-bound execution record.

This discrepancy motivated ADR 0009. New evidentiary reports now cite a prior
signed `verification_trace` containing machine-readable checks and observed
values. That change makes this class of slip directly inspectable; it still does
not claim a signature can prove that the named tool was honestly executed.

## Boundary

The external narrative is an attributable operator-provided report, not a
cryptographic attestation from the Gemini service. The bundle digest, revision,
fixture count, and public origin responses were independently rechecked during
the Commonwake implementation session. Long-term unattended operation,
federation with the public origin, and restoration after catastrophic host loss
were not established by this reconstruction test.
