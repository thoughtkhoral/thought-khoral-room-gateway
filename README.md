# ThoughtKhoral room gateway

`thought-khoral-room-gateway` is the Rust/Axum service that authenticates room
participants and validates, persists, replays, and broadcasts governed
ThoughtKhoral room events.

## Status

MVP / active development. The gateway owns the authenticated room boundary; it
does not define the shared contract or provide the local platform composition.

For the cross-project Codex conversation status and remaining gates, see the [shared status guide](https://github.com/thoughtkhoral/thought-khoral/blob/main/docs/codex-conversation-status.md).

## Build and verify

Requires Rust 1.88 or newer and a PostgreSQL-compatible development database for
integration tests:

```sh
cargo fmt --check
cargo test
```

See the [local specification index](.ai/specs/README.md), the
[repository map](https://github.com/thoughtkhoral/thought-khoral/blob/main/docs/repository-map.md),
and the [organization contribution guide](https://github.com/thoughtkhoral/.github/blob/main/CONTRIBUTING.md).

## Runtime identity and configuration

`GET /health` returns the active product name `ThoughtKhoral`, the service
identifier `thought-khoral-room-gateway`, and an `ok` status. Startup logs use
the same product and service labels.

Set `DATABASE_URL` plus the following service configuration:

- `THOUGHT_KHORAL_ALLOWED_ORIGINS`
- `THOUGHT_KHORAL_LISTEN_ADDRESS` (defaults to `127.0.0.1:8080`)
- `THOUGHT_KHORAL_OIDC_ISSUER`
- `THOUGHT_KHORAL_OIDC_AUDIENCE`
- `THOUGHT_KHORAL_OIDC_JWKS`
- `THOUGHT_KHORAL_SESSION_AUTH_TIMEOUT_MS` (defaults to `5000`)

Legacy `N2N_*` configuration aliases are intentionally not accepted: a missed
deployment rename must fail startup instead of silently selecting an old
runtime resource.

## Compatibility exclusions

The vendored `n2n.room.v1` contract is pinned to its authoritative commit (recorded in
`contracts/lock.json` with source archive and schema hashes). Its JSON Schema identifiers, the existing `n2n_role` JWT claim, and the
database schema/data identifiers remain unchanged for wire and data
compatibility. They require dedicated contract or data migration decisions
before they can be renamed.

For the complete boundary and local decisions, see [the local specification
index](.ai/specs/README.md).

## Decision management

Human participants use the workspace `/decisions` workflow for governed
Create, Update, and Delete actions. `decision.delete` physically removes the
current decision row while retaining its `decision.deleted` audit event and
request-ledger entry. A chat message beginning with `Decision:` is ordinary
chat; it does not trigger an automatic facilitator proposal. The gateway keeps
the propose-only facilitator port for a separately approved memory-derived
draft implementation. See [the slash-decisions decision](.ai/specs/decisions/003-slash-decisions-crud.md).

## Local A2A task mediation

The earlier `@action-items` mention invokes the in-process deterministic
Action Items Agent. Separately, a human `agent.task.start` invokes one pinned
local A2A reference agent with either `summarize-context` or
`extract-action-items`. The room gateway owns the durable task, authorizes and
filters the full ordered room-context snapshot, issues a bounded worker lease,
and validates cited results and terminal state before persisting room events.
The agent gateway uses a separate authenticated internal task interface; it
does not receive database credentials or active-decision authority. The local
platform and [A2A foundation specification](https://github.com/thoughtkhoral/thought-khoral-agent-gateway/blob/main/.ai/specs/what/a2a-agent-gateway-foundation.md)
describe the reference integration. Remote agent admission is not enabled.

## Message mentions and delivery

The gateway is authoritative for mention delivery. It validates every direct
target against the current room roster and canonical display-name token,
rejects unknown, stale, duplicate, or noncanonical targets without writing an
event, and expands fixed aliases by role. `@allhumans` reaches humans only;
`@allagents` reaches agents and is also visible to all humans for supervision.

Messages default to room-wide delivery. `delivery: "mentioned"` persists a
resolved `audienceIds` list containing the sender and is filtered consistently
for live broadcast and replay while global room sequence numbers continue to
advance. The vendored `n2n.room.v1` contract documents the wire fields and is
pinned in [`contracts/lock.json`](contracts/lock.json).

## Codex conversation routes and storage

The approved [conversation design](.ai/specs/how/codex-room-participation.md)
adds an independent contract pin in
[`contracts/agent-conversation-v1/lock.json`](contracts/agent-conversation-v1/lock.json),
strict turn validation, migration `0006_agent_conversations.sql`, and atomic
conversation reservation. A reservation stores one ordinary human message and
one task with frozen public context, selected settings and a context digest.
Targeted messages and decisions with targeted or missing sources are excluded.
Retries preserve the original acceptance; concurrent turns cannot reserve the
same room/agent generation. Storage tests require a disposable, migrated
PostgreSQL database via `DATABASE_URL`.

Conversation policy defaults to disabled. `THOUGHT_KHORAL_CODEX_POLICY_JSON`
parses the closed deployment policy described in the governing design. An
enabled policy requires reviewed policy, guidance and catalog revisions plus
allowed model/effort pairs and defaults. Configuration errors fail startup.
The separate `/api/agent-conversations/v1` browser routes now accept turns and
expose conversation, task and mediated catalog views. The additive
`GET /api/agent-conversations/v1/rooms/{roomId}/agents/{agentId}/defaults`
resolves the current enabled deployment model/effort pair against the mediated
catalog in five seconds, under current human authority. It accepts no query,
works for a valid unused room, creates no stored rows, and returns the closed
`ResolvedSettingsView` with `Cache-Control: no-store`. An unavailable pair or
bridge returns `runtime_unavailable`; an explicit still-valid submitted pair
continues to govern after only deployment defaults change. The endpoint uses
the [unreleased v1.1 candidate lock](contracts/agent-conversation-v1.1-candidate/lock.json)
beside the unchanged published vendor, as authorized by the
[defaults-discovery amendment](.ai/specs/how/codex-room-participation.md#approved-defaults-discovery-amendment--2026-10-07).
The candidate does not claim released interoperability. The
`/internal/agent-conversations/v1` routes use admitted workload authentication
for claims, frozen context, updates, authority and receipt recovery. Migration
`0007_conversation_lifecycle.sql` stores normalized lifecycle projections and
immutable replay records. Accepted replies atomically commit one ordinary room
message, its acknowledgement, the cursor and disclosed-source manifest.

The executable supplies the fixed authenticated agent-gateway catalog bridge
when the Codex policy is explicitly enabled with its dedicated bearer secret;
an absent or unavailable bridge rejects new work. Policy stays disabled by
default. Provider verification remains separately gated. HTTP integration tests
use synthetic isolated PostgreSQL schemas and a fake external catalog; bridge
tests use a real synthetic HTTP listener, with no provider calls.

The opt-in Codex policy requires a dedicated catalog bridge bearer secret. The
fixed mediator endpoint, bounds, local pagination and dependency review are
defined by the [Task 6 design checkpoint](.ai/specs/how/codex-room-participation.md#task-6-broker-catalog-bridge-checkpoint--2026-10-06).
Default configuration remains disabled.
