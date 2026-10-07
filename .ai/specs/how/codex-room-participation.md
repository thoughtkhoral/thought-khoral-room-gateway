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

## Task 6 broker catalog bridge checkpoint — 2026-10-06

The executable creates the reviewed `CatalogQuery` bridge only when the Codex
policy is explicitly enabled. Enabling requires the dedicated
`THOUGHT_KHORAL_CODEX_CATALOG_BRIDGE_SECRET`: 32–4096 ASCII bearer-token bytes
(alphanumeric or `-._~+/=`), distinct from detectable existing worker invocation,
reference agent, OAuth client and memory engine shared credentials. Disabled
policy creates no bridge and requires no secret. The opaque secret has no
serialization and only redacted Debug; its Authorization header is sensitive.

The sole destination is
`http://thought-khoral-agent-gateway:9092/internal/agent-conversations/v1/models`.
GET carries no query or body, uses no proxy, follows no redirects, and has a
two-second connect/five-second total deadline. DNS resolves only the fixed
service hostname; loopback DNS injection is compiled for tests only. Response
reads reject declared or streamed bodies above 1 MiB. Strict JSON parsing
preserves duplicate-key and integer rules and validates the published closed
CatalogPage before exposure. Upstream always supplies the full page, maximum
100 models with no upstream cursor. Local opaque pagination obeys limits 1–100;
its maximum 1024 cursor entries bind SHA-256 of the validated full page and
an offset. Unknown cursors fail before network access; changed content or
revision invalidates continuation. Existing browser policy validation still
checks catalog revision, duplicate models/efforts, default effort membership
and deployment selection allowlists. No provider or worker invocation credential
is supplied to this bridge.

The direct HTTP dependency is exactly reqwest 0.13.5, matching the mediator
lock. Default features are disabled because the fixed internal endpoint is
HTTP; no TLS/provider transport is introduced. New packages below come from
crates.io with Cargo.lock archive SHA-256 provenance and registry Cargo.toml
license declarations. Existing locked package versions remain unchanged.

| Package | Version | License | Archive SHA-256 |
| --- | --- | --- | --- |
| base64 | 0.23.1 | MIT OR Apache-2.0 | ac07cdecf99051d9a5238b80f35af32cdeba5b336e55d957b318b50137e18da5 |
| ipnet | 2.12.2 | MIT OR Apache-2.0 | 791930b43c0d5973160d90a8f3894509f2b273430f5c5c73b668636d0287c5c0 |
| reqwest | 0.13.5 | MIT OR Apache-2.0 | 16a1cfa75cc186dd73d5818e510e042e40927bccc9c236b061cea97e1eb08029 |
| tower-http | 0.6.11 | MIT | 4cfcf7e2740e6fc6d4d688b4ef00650406bb94adf4731e43c096c3a19fe40840 |
| try-lock | 0.2.5 | MIT | e421abadd41a4225275504ea4d6566923418b7f05506fbc9c0fe86ba7396114b |
| want | 0.3.1 | MIT | bfa7760aed19e106de2c7c0b581b509f2f25d3dacaf737cb82ac61bc6d760b0e |
| wasm-bindgen-futures | 0.4.78 | MIT OR Apache-2.0 | 6ef4c5d3d2cdf5c54f4231181768f5510842e350db025faf1f7163b1030ed928 |
| web-sys | 0.3.105 | MIT OR Apache-2.0 | 9fbddc4a036f00ec4f18c83445bd3115cb306a91da554919a099d9222fe4a7f8 |

Verification is recorded in the accompanying Task 6 report; provider calls,
service activation, publication and deployment remain separately gated.

## Task 9 synthetic verification and correction checkpoint — 2026-10-07

The following defaults-discovery amendment status supersedes this pre-amendment F1/default-discovery disposition; Task 9 remains open for release gates.

Provider-free checkpoint only; Task 9 and the milestone remain open.

Task-scoped verification review: Approved. Broad implementation review: Partial
spec compliance; quality Needs follow-up. B1–B3 (pending-ack recovery, omitted
shared settings and receipt-correlated safe failures) are addressed. B4 is
partial: explicit initial/reset selection works for full-capability admission,
but automatic server-default display requires an approved interface amendment.
F1: unresolved Important reasoning-only UI deadlock. Optional capabilities are
independent; an effort-only admission cannot establish the guard-required model
through its hidden selector. This prevents initial/reset invocation and blocks
whole-milestone/merge readiness. No second broad fix wave or waiver is implied.

| Owner | Final reviewed local revision |
|---|---|
| contracts | `85baf86e574276fcd036e53e23641af6aad602f9` |
| broker | `fd05cb48b8508e7939f9cdf9df275742a06fc4f8` |
| mediator | `6c3d96b4763871b9addc9bc7223e71ee7d38abd9` |
| worker | `b0d43ec2b5b0c8da035d4ccff754545132b978d4` |
| ui | `e51d67e9e1a986601df6b5e1acf68aaf7ae0870d` |
| platform | `637a69279f0fe5019560b1e54d28f48c1c715897` |

All six reviewed worktrees were clean when this checkpoint was prepared.
Runtime is committed only on isolated local branches; originals retain their
runtime/scaffold and unrelated edits. Contracts v1.0.0 and dependency lockfiles
remain unchanged.

Controller final verification on platform revision above: `node
scripts/smoke-codex-conversation.mjs --fake` (session85023) exit0, six original
crash/commit boundaries, 11 fake native turns, exact baseline/delta/source IDs,
targeted/cross-room exclusion, duplicate=one logical turn, worker restart and
fresh reset, shared omitted settings, rejected-completion recovery and exact
execution_failed/session_unavailable/runtime_unavailable projections. `node
--test scripts/tests/codex-conversation-smoke.test.mjs` (session23304) exit0,
13 passed, zero failed. This is synthetic native/identity/private-DNS adapter
coverage, not whole packaged Compose, real Keycloak/browser or provider proof.

Inspected owner logs and independent review record broker150 passed + one
pre-existing ignored live test; mediator65 passed, zero failed/ignored (correcting
the earlier reported68); worker46; UI121 + pin/tamper checks and production
build. Owner fixture tests3, actual assertion-failure/SIGTERM/SIGINT cleanup3,
package/startup checks10 passed. Earlier contracts/regression/legal/pin evidence
is retained with original attribution, not presented as rerun here.

New ARM64 worker image:
`sha256:2d8bfade27802f910cf68e832722c93b4a2acc2addb825711e1223617a4cd385`.
Compiled runtime revision `b418a76e0e7ca047b5fe995eb17519aced369a06`; worker
head above adds evidence documentation. Immutable image readiness checks used
network-none/read-only/cap-drop-all, both admission markers; default invocation
refused as expected. Actual native 44-setting/eight-model/resume/six unsolicited
tool refusal evidence remains attributed to its earlier source/image, not this
new image. CLI/catalog/control hashes are unchanged. x86_64 native admission and
Rust1.85 minimum-version checks remain unrun.

Default discovery: pending specification approval. The local proposed How is
`thought-khoral-codex-agent/.ai/specs/how/default-settings-discovery-proposal.md`.
It proposes a read-only authenticated defaults query in a new immutable v1.1.0
artifact and independent mixed-capability controls, covering absent conversation
and explicit New/reset. It authorizes no runtime or published contract changes.
Live provider verification: pending. Account/model availability, actual native
history, live tool/egress/key isolation, packaged deployment/private DNS and real
browser/identity evidence remain separately gated. No merge, push, publication,
service activation or provider inference occurred.

Independent review artifacts are retained outside Git at
`/private/tmp/codex-conversation-task9/final-fix-review.md`,
`final-implementation-review.md`, and `task9-fix-review.md`; owner evidence at
`/private/tmp/Task9-final-fix-evidence/`. Final root/documentation/source-reference
and identity gate results will be recorded in the controller checkpoint after
these source-derived record updates. The aggregate release checklist remains
unchecked; passing synthetic checks do not resolve F1 or default discovery.


The approved parent shared-default rule governs settings omission: continuation uses the persisted accepted pair after current validation; fresh New without explicit settings uses deployment defaults. Earlier configured-default wording applies to fresh New, not continuation.

## Approved defaults-discovery amendment — 2026-10-07

The maintainer approved the [visible server defaults design](https://github.com/thoughtkhoral/thought-khoral-codex-agent/blob/main/.ai/specs/how/default-settings-discovery-proposal.md) in
this conversation on 2026-10-07 after an explicit specification approval request.
It authorizes coordinated local implementation and synthetic verification of
the additive authenticated defaults query and independently optional model/effort
controls, including the F1 initial/reset effort-only deadlock. The accepted
design is the governing amendment to earlier default-visibility wording.

The contracts owner defines `ResolvedSettingsView` at
`GET /api/agent-conversations/v1/rooms/{roomId}/agents/{agentId}/defaults` in
new immutable artifact `thought-khoral-agent-conversation-v1.1.0`, retaining the
v1 profile/namespace and all existing published v1.0 schema/fixture bytes.
The broker validates authenticated room/agent authority, current admission,
catalog revision, policy-default pair and five-second bound before responding.
The read has no task/event/conversation/lease/native-state mutation, exposes no
effective-settings confirmation, credentials or private/native identifiers,
uses the existing safe ProfileError/HTTP mapping and `Cache-Control: no-store`.
There is no inferred catalog-order model or inference fallback.

The UI resolves and displays the concrete explicit next-turn pair when absent
or explicitly New/reset; restored continuation uses accepted shared settings.
Both capabilities allow both controls; effort-only keeps the resolved model
read-only; model-only keeps the displayed model-specific catalog default effort
read-only; neither capability retains the settings-free path. Unsupported
controls stay uneditable and no hidden control blocks a valid required choice.
Catalog/pair mismatch requires bounded refresh or an explicit unavailable state.
A still-valid explicit pair is not replaced after a deployment-default-only change.

As a scoped exception to the earlier published-artifact-first execution order,
isolated consumers may pin a reproducible local candidate from an exact committed
contracts revision, verified archive and per-file SHA-256 values, clearly marked
unreleased. This exception is only for this amendment's local pre-publication
development and synthetic testing. Published v1.0 provenance/bytes remain intact.
No release publication, shipped interoperability, merge, push, provider use or
service activation is authorized. Whole milestone/Task9 acceptance remains open.

## Defaults discovery local synthetic checkpoint — 2026-10-07

All four defaults-amendment tasks passed their independent reviews. The final
whole-branch review passed. F1 (initial/New reasoning-only settings deadlock) and
visible defaults discovery are accepted for this local synthetic candidate.

| Source | Exact local revision | Retained worktree |
| --- | --- | --- |
| contracts | `1ea828f28725ddaaefa21d083473f9abbd777975` | `/private/tmp/codex-conversation-defaults/contracts` |
| broker | `2e7d23b467c572819f498c3b9bf14d74a62dc821` | `/private/tmp/codex-conversation-defaults/room-gateway` |
| mediator | `6c3d96b4763871b9addc9bc7223e71ee7d38abd9` | `/private/tmp/codex-conversation-final-fix/agent-gateway` |
| worker | `b0d43ec2b5b0c8da035d4ccff754545132b978d4` | `/private/tmp/codex-conversation-final-fix/worker` |
| ui | `79e5e7310a450efea561548cd87871446c1939aa` | `/private/tmp/codex-conversation-defaults/workspace-ui` |
| platform | `2d856773078a7caa542d719e539b55a5ab2dafaa` | `/private/tmp/codex-conversation-defaults/platform` |

The composed run was executed at `f9afeb20b746200daa9cdef0406c03f88a422b68`.
The subsequent path-provenance correction was tested and scoped-reviewed at
`220f6e0c74a29c000d7de81c0cb77823de0bd15c`;
the final platform revision above adds completion metadata only. The original
repositories retain their runtime; local implementation branches remain unmerged.

The unreleased candidate contract source is
`1ea828f28725ddaaefa21d083473f9abbd777975`, proposed release
`thought-khoral-agent-conversation-v1.1.0`. Its archive SHA-256 is
`fab59a486f6498b843467202debcb0768403bd57ba7dda41be2a01e5f23fdda8`
and externally anchored lock SHA-256 is
`7914d32eae2487879a68405b5095a6b9aa91355f87529c43f4055844821902a9`.
All 156 candidate payload files match in broker/UI; published v1.0 bytes remain
unchanged. The profile and API namespace stay v1. This is not a published release.

Evidence: retained 125 contract fixtures plus 16 additive cases and 6 candidate
integrity tests; broker serial suite 158 passed with 1 existing live-only test
ignored; UI full suite 190 passed with 1 intentional composed skip, followed by
scoped harness/TypeScript checks; 26 source-pin and 13 retained runner guards;
5 fixture unit tests; 3 actual failure/SIGTERM/SIGINT cleanup cases. The composed
candidate passed 12 capability/lifecycle cases, 3 display/send mutation cases,
15 actual HTTP UI children with 90 test passes, and 24 synthetic native turns
(11 retained baseline plus 13 added). Stale/removed pairs allocate no task/event
or native turn, retain the prompt and require explicit Refresh. Default-only
changes preserve the displayed explicit pair; unavailable replay is immutable.
Paused-publisher regressions verified RED before and GREEN after atomic exclusive
handshake publication. Owned processes, containers and staging files were cleaned.

The current composed state is `/var/folders/70/5kxy5kys3bj0252chp3ck8900000gn/T/Task9-codex-conversation-j9py6w`. Full provenance,
task/thread bindings, raw log references, limitations and review reports remain in
`/private/tmp/codex-conversation-defaults/defaults-reviewed-checkpoint.json` and
`/private/tmp/codex-conversation-defaults/task-4-logs/`. Root hierarchy/reference/
identity, scaffold documentation and whitespace results are recorded separately
in `/private/tmp/codex-conversation-defaults/final-gates.json` after synchronization.
The old parallel broker fixture port collision and Vite chunk advisory are
retained limitations; no passing parallel broker-suite claim is made.

Publication: pending

Packaged-stack verification: pending

Live provider verification: pending

The composed gate uses jsdom, a synthetic room socket, actual conversation HTTP
and storage, and a fake native executable. It does not establish packaged Compose,
real browser/Keycloak, provider, architecture-minimum or new-image acceptance.
Task 9 and milestone aggregate gates remain open. Specification/memory-guided
working directories remain the separately scoped future extension.

## Defaults-discovery amendment status — 2026-10-07

The approved amendment was implemented on the reviewed local broker candidate
`codex-defaults-broker` at `2e7d23b467c572819f498c3b9bf14d74a62dc821`.
The broker participates in read-only resolution of the defaults view without
changing the retained room-event contract. The candidate was included in the
reviewed composed synthetic run; the default branch still retains its prior
runtime, and no candidate contract release, packaged-stack verification or
authorized live-provider check has occurred. See the [coordinated checkpoint](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/codex-room-conversations-implementation-plan.md).
