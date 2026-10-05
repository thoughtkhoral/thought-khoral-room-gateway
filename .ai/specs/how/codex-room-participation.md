# Codex Room Participation — implementation design

## Status

Approved by the project maintainer in the Codex working session on 2026-10-05,
including this milestone-one specification and the coordinated implementation
plan. Accepted contribution: [issue 1](https://github.com/thoughtkhoral/thought-khoral-room-gateway/issues/1).
Implementation follows the [plan](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/codex-room-conversations-implementation-plan.md) and its dependency gates.
Release/tag publication, provider use and service activation require their
separate later authorization. No completed runtime or live verification is claimed.

## Governing sources

- [Local requirements](../what/codex-room-participation.md)
- [Root solution design](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/codex-chat-agent.md)
- [Exact profile](https://github.com/thoughtkhoral/thought-khoral-contracts/blob/thought-khoral-agent-conversation-v1.0.0/.ai/specs/how/agent-conversation-profile.md)
- [Repository tasks and gates](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/codex-room-conversations-implementation-plan.md)

## Design

Add separate conversation HTTP/workload routers and tables alongside current
agent_service and retained room handlers. Authenticate browser bearer tokens with
the same OIDC validator and allowed-origin policy; use existing workload identity
for the mediator. Current human-role/room authorization remains the source of
permission: do not invent durable membership or treat socket Leave as revocation.
Freeze input at the invoking message sequence. Store a generation disclosure
manifest and advance its cursor only with the accepted reply transaction.
Accepted replies are existing room-wide message.created events; profile task
updates are in separate task storage. Existing deterministic leases and exact
results are untouched. Poll authority during active turns; token expiry, agent
revocation, or policy narrowing denies terminal submission and requires safe
interruption. No database credentials are given to the worker.

## Verification

Use the exact contract fixture cases, root What acceptance criteria, and assigned
repository tasks in the plan. Scope tests to real protocol/storage/UI behavior;
fake only the provider/app-server boundary where a live dependency is unnecessary.
Publication/runtime execution requires accepted issue links and written review.
Do not claim live model, history, sandbox, or egress coverage from schema tests.


## Task 2 decomposition and local execution

Task 2 was authorized by the maintainer on 2026-10-05 after contract publication.
Vendor the published schema/fixture directory with an independent lock; the
retained contract lock stays unchanged. ConversationStore defaults to disabled;
its explicit deployment policy enables only the pinned Codex identity, allowed
model/effort pairs, catalog/policy/guidance revisions and configured lower bounds.
No HTTP route, agent registration or live process is activated by this task.

Reservation reuses the existing transaction-scoped room lock and room/request
ledger so a profile request cannot collide with ordinary chat or deterministic
work. Authenticate the human role and bound token expiry before even returning
an idempotent response. Existing authorization admits an authenticated human
under the current room model; there is no durable membership or socket-presence
requirement. A future ACL replaces this authorization port.

The store freezes the context/settings core and validity timestamps in the
reservation transaction. Lease owner/expiry/token are assigned by Task 3 claim
handling; the reserved core is private storage, not a claimable wire TaskInput.
A frozen digest must be computed before commit. A private snapshot helper owns
bounded public transcript and decision projection for this storage unit; Task 3
extracts the context module and adds workload delivery and result transitions.
It must not reconstruct a changed context outside the original transaction.

All tables enforce room/agent/generation/task binding with foreign keys/checks;
only one non-superseded generation exists per room/agent. Disclosure manifests
and accepted native-reply acknowledgements are generation scoped. A replacement
is superseded/reserved atomically; failures leave the preceding state unchanged.
Tests use a disposable migrated PostgreSQL database, never platform room data.

### Opt-in policy fields

The optional `THOUGHT_KHORAL_CODEX_POLICY_JSON` value is a closed object with
`enabled` (default false), `policyRevision`, `guidanceRevision`,
`catalogRevision`, `model`, `reasoningEffort`, and `models`. Each model is the
closed object `{id, reasoningEfforts}`. Enabling requires nonempty revisions
and option IDs of at most 128 Unicode scalars, unique model/effort IDs, an
allowed default pair, and at most 100 models/efforts per model. Submitted
settings must match the current catalog revision and an allowed pair; omission
uses current configured defaults. Stored replay is compared before resolving
these mutable defaults.

Optional lower bounds are `maxContextBytes` (default 1,048,576), `maxRecords`
(default 2,000 transcript entries plus native-reply bindings),
`turnDeadlineSeconds` (default 180) and `interruptGraceSeconds` (default 5).
Each is positive and cannot exceed its profile default. Active decisions also
contribute to the canonical byte cap. Preflight counts only a lower bound of
disclosed fields, excludes unrelated message metadata, and bounds native-reply
verification reads. The final canonical preimage byte count is authoritative.

A ready generation requires a positive committed consumed revision. Update
ordinals are positive safe integers and preserve coalescing gaps; the later
service limits retained update rows to 256, rather than capping the ordinal.
The human permission port clamps the supplied expiry to the validated Actor's
token expiry and checks authority before replay, after lock acquisition and
before commit. It does not add durable membership to the current room model.

### Task 2 verification record — 2026-10-05

The isolated `codex-conversation-storage` branch starts at `dca68a8` and consumes
the published contract commit `85baf86e574276fcd036e53e23641af6aad602f9`.
All 135 vendored schema/fixture files match their independent lock hashes and
the downloaded release artifact. The retained contract and Cargo dependency
lock remain unchanged.

Tests ran against a dedicated PostgreSQL 16 container with tmpfs storage and
synthetic public/targeted examples, without platform data or provider access.
The complete `cargo test --locked --all-targets` run passed 121 tests; the
existing live reference-agent test remains ignored because its external binary
is not running. A first full run encountered one existing room-flow WebSocket
timeout; that target passed all 23 tests on unchanged rerun, and the final full
run passed. The final focused library/validator/store check covers the Clippy
cleanup after the full run. Formatting, Clippy with warnings denied, and
`git diff --check` pass.

Independent read-only review found and corrected inaccurate context byte
preflight and the update ordinal limit. Regression tests first reproduced both
findings and the zero-cursor state, then passed on a freshly recreated migration.
Final review reports no remaining Critical or Important findings. Root
specification hierarchy, reference validation/regressions and identity
validation/regressions pass, including an isolated snapshot of this checkout.
No conversation routes, claims, result transitions or live Codex execution are
activated by this storage unit; these remain later tasks.
