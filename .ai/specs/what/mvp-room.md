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
- It validates direct chat mentions against canonical tokens from the current
  room roster before persistence. Room-wide messages reach all participants;
  mentioned-only messages reach the resolved targets and sender. Fixed
  `@allhumans` and `@allagents` aliases expand by role, with all humans able to
  supervise `@allagents` messages. Live delivery, replay, and agent context
  snapshots exclude messages outside an actor's persisted audience without
  renumbering global room events.
- Only a human participant can confirm, edit, or dismiss a draft decision or
  delete an existing decision. Deletion removes the current row but retains a
  complete immutable audit event; no agent proposal affects active context.
- The gateway owns the facilitator draft-proposal port. After a persisted room
  event, that port may return zero or more drafts which the gateway records as
  `decision.proposed`. The port must not invoke a decision transition, write
  active context, or call an external model, tool, or operating-system command
  inside this process. See root [decision 005](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/005-room-scoped-poc-memory.md).
- The former deterministic `Decision:` parser is retired: prefixed text is
  ordinary chat. The facilitator port remains the only path for future
  memory-derived drafts from persisted events; a memory-engine / Cognee
  implementation is out of this project's runtime until a separately
  approved plan enables it. Human `/decisions` creation uses governed
  `decision.propose` directly and is not a second derived-draft proposer.
  See root [decision 008](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/008-slash-decisions-and-facilitator-boundary.md).
- The gateway registers the deterministic Action Items Agent and, after a
  human room-wide direct mention, atomically records the source message and
  replayable task lifecycle
  events. The executor has no model, tools, filesystem, shell, network, or
  direct database access and cannot affect decisions or active context.
- For the separate local A2A reference task, the gateway authorizes a human
  `agent.task.start` for one pinned agent and two skills, leases the task to an
  authenticated worker, and assembles a task-bound snapshot of the full
  ordered history visible to both requester and agent plus active decisions.
  It validates context revision, lease, update identity, citations, and
  terminal state before persisting and broadcasting normalized task events.

## Interfaces

The gateway consumes the `n2n.room.v1` contract method
`session.authenticate` for browser connection establishment and the
authenticated methods `room.join`, `chat.send`, `decision.propose`,
`decision.transition`, `decision.delete`, and `agent.task.start`. It exposes
their normalized events, participant snapshots/updates, and errors over its
WebSocket, governed by [root Decision 002](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/002-browser-websocket-authentication.md)
and the accepted [local browser profile](../decisions/002-browser-session-authentication.md).
Its separate authenticated internal task interface supports lease claim,
context retrieval, and normalized update submission; it does not expose a
browser A2A endpoint.

The gateway is authoritative for `chat.send` mention resolution and
`message.created` audience persistence under the root [message delivery design](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/message-mentions-and-delivery.md).

## Explicit exclusions

This project does not own contract definitions, provide a browser UI, compose
local infrastructure, grant agents database credentials or shell access,
integrate Cognee or other memory-engine runtimes in this process, or host
external agent runtimes, A2A transport, or MCP services. Remote agent
admission remains deferred; the local reference path is mediated by the
separate agent gateway.
