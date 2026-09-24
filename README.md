# ThoughtKhoral room gateway

`thought-khoral-room-gateway` is the Rust/Axum service that authenticates room
participants and validates, persists, replays, and broadcasts governed
ThoughtKhoral room events.

## Status

MVP / active development. The gateway owns the authenticated room boundary; it
does not define the shared contract or provide the local platform composition.

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
