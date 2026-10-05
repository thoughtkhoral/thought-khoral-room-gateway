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

## Task 3 execution decomposition

Task 3 was authorized by the maintainer after the verified Task 2 checkpoint.
Extract the transaction-bound snapshot and canonicalization into
`conversation_context`; retain the exact frozen input and source manifest.
Add migration `0007_conversation_lifecycle.sql` for lifecycle projections,
last ordinal/progress time and a separate immutable update-replay ledger.
Retained/coalesced browser updates are bounded to 256 while replay fingerprints
remain durable and terminal state immutable.

Register separate browser and admitted-mediator routers. Browser bearer identity
and configured origin checks precede input decoding; CORS preflight grants no
room authority. Decode JSON with duplicate-key detection and bounded bodies.
Only the room/agent policy enables these handlers; defaults remain disabled.
The authenticated catalog mediation port returns the published normalized
CatalogPage through the agent gateway boundary, never fabricated provider
availability. An absent/unavailable catalog bridge fails closed. Task 6 connects
the reviewed mediator implementation; provider-free Task 3 tests supply only
that external capability boundary. Runtime selections must match both the
current mediated catalog and deployment allowlist, before reserving new work.
An idempotent accepted retry compares intent and returns its original settings
before fetching changed catalog defaults.

Claims mint a fresh lease token for the pinned agent and never reclaim possibly
submitted work without receipt reconciliation. A running expired lease fails
interrupted and makes the generation unusable. Unknown native history is never
restored. Context/updates/authority enforce lease, generation, frozen digest,
deadline, requester token expiry and current policy/guidance. Authority success
is the closed object `{profileVersion, taskId, generation, contextDigest,
expiresAt, authorizationExpiresAt}`. A receipt is the closed object
`{profileVersion, taskId, conversationId, generation, state, acknowledgement,
result}`; only completed tasks expose the acknowledgement and result. Receipt
reads are scoped to the authenticated mediator and pinned agent but require no
live lease. They cannot authorize a new execution.

Commit a terminal reply, update, consumed cursor, generation disclosure manifest
and broker acknowledgement together under the existing room lock. Publish only
ordinary room message events after commit; retries never rebroadcast. Check
citations against currently public disclosed source IDs, including the frozen
packet and previously committed generation manifest. Failed updates release
the reservation as unusable when unchanged native history cannot be proved.
Live worker/provider execution, deployment and the guided workspace stay outside
Task 3. No browser Leave, durable membership or retained protocol changes are added.

Task 3 preserves the first accepted progress update even if it arrives after a
working update; subsequent accepted progress is coalesced. Browser polling
reads task and updates from one repeatable-read snapshot. Final authority is
rechecked before commit. Terminal update retries require a live lease and current
generation/policy; receipt reads provide historical acknowledgement recovery.
Confirmed settings require nonnull model and effort; normalized usage with no
window must be unavailable, as required by the published semantic annex.

A nonterminal update at the maximum safe integer cannot leave a representable
terminal successor and is rejected before persistence as invalid input. This
lifecycle-exhaustion guard applies only to nonterminal updates at that boundary;
terminal maximum ordinals and arbitrary increasing ordinal gaps remain valid.
It does not replace the separate 256 retained-row bound with an ordinal cap.
The boundary is not explicitly distinguished in the published profile and must
be carried into its next semantic clarification before cross-language activation.

Additional direct mentions are checked only for a new reservation, after the
immutable request ledger lookup. Canonical roster changes therefore cannot
invalidate accepted intent replay. Mention checks use a latest-actor identity
projection with no room payloads; pinned-Codex-only requests need no roster
query. Additional-target admission rejects projections exceeding 2000 actors
or an 8000-byte display name rather than reading unbounded history or silently
truncating names. Presence snapshots obey the same bounds. Disabled policy and
accepted replay bypass this new-work projection.

### Task 3 verification record — 2026-10-05

The isolated `codex-conversation-routes` branch extends Task 2 commit
`4f4d014194a7887c06722215181fd7451ca50f55`. The final
`cargo test --locked --all-targets` passes 142 tests against a dedicated
PostgreSQL 16 container with tmpfs data and synthetic fixtures. The existing
live reference-agent test remains ignored because its external binary is not
running. New targets contain three literal canonicalization tests and 18 real
HTTP/database service tests. Per-server schemas prevent claims from crossing
test boundaries. No platform data or provider calls were used.

`cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, and
`git diff --check` pass. The independent contract lock/hash tests pass; retained
contract and Cargo locks are unchanged. Root specification, reference and
identity checks and regressions pass, including the isolated gateway snapshot.
The independent review findings were reproduced and corrected with regressions;
final read-only review reports no remaining Critical or Important findings.
The maximum nonterminal ordinal boundary still requires the explicit published
profile clarification documented above before activation.

This unit delivers opt-in profile routes, bounded frozen baseline/delta context,
workload claim/authority/receipt paths, normalized task polling and atomic public
reply commit. Retained ordinary chat does not invoke Codex. The executable has
no catalog bridge and policy defaults to disabled. Task 4 implements the
independent app-server adapter; Task 6 connects the mediator/catalog bridge.
Publication, provider use and activation retain their separate gates. Browser
Leave and durable room membership were not added.
