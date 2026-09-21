# MVP room gateway

## Sole MVP responsibility

`thought-khoral-room-gateway` is the sole mediator that validates, persists, replays, and broadcasts governed room events and active-context updates.

## Acceptance criteria

- `GET /health` identifies the product as `ThoughtKhoral`, the service as
  `thought-khoral-room-gateway`, and its status as `ok`.
- The gateway validates supported `n2n.room.v1` JSON-RPC requests and returns the specified structured errors for rejected requests.
- For an allowed browser Origin, the gateway accepts only `session.authenticate` before binding a fully validated OIDC identity, and closes authentication failures or timeouts without admitting a room operation.
- It persists ordered immutable room events before broadcasting them and supports replay after a sequence cursor.
- It returns the room participant snapshot from `room.join` and broadcasts ephemeral participant presence updates, using trusted OIDC display-name claims when available and a role-plus-short-ID fallback otherwise.
- Only a human participant can confirm, edit, or dismiss a draft decision; no proposal affects active context without that action.
- The gateway owns the facilitator draft-proposal port. After a persisted room
  event, that port may return zero or more drafts which the gateway records as
  `decision.proposed`. The port must not invoke a decision transition, write
  active context, or call an external model, tool, or operating-system command
  inside this process. See root [decision 005](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/005-room-scoped-poc-memory.md).
- The live implementation of that port is the deterministic `Decision:` parser:
  it may derive a draft only from a persisted `message.created` event whose
  trimmed text starts with `Decision:` and has a non-empty remainder. It
  attributes that proposal to the gateway agent and retains the triggering
  event identifier as provenance. A memory-engine / Cognee implementation of
  the same port is out of this project's runtime until a separately approved
  plan enables it; this gateway must not add a second independent propose path.
- The gateway registers the deterministic Action Items Agent and, after a
  human room-wide direct mention, atomically records the source message and
  replayable task lifecycle
  events. The executor has no model, tools, filesystem, shell, network, or
  direct database access and cannot affect decisions or active context.

## Interfaces

The gateway consumes the `n2n.room.v1` contract method `session.authenticate` for browser connection establishment and the authenticated methods `room.join`, `chat.send`, `decision.propose`, and `decision.transition`. It exposes their normalized events, participant snapshots/updates, and errors over its WebSocket, governed by [root Decision 002](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/002-browser-websocket-authentication.md) and the accepted [local browser profile](../decisions/002-browser-session-authentication.md).

## Explicit exclusions

This project does not own contract definitions, provide a browser UI, compose
local infrastructure, grant agents database credentials or shell access,
integrate Cognee or other memory-engine runtimes in this process, or integrate
external agent runtimes, A2A, or MCP services.
