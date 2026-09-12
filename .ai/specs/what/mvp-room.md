# MVP room gateway

## Sole MVP responsibility

`thought-khoral-room-gateway` is the sole mediator that validates, persists, replays, and broadcasts governed room events and active-context updates.

## Acceptance criteria

- `GET /health` identifies the product as `ThoughtKhoral`, the service as
  `thought-khoral-room-gateway`, and its status as `ok`.
- The gateway validates supported `n2n.room.v1` JSON-RPC requests and returns the specified structured errors for rejected requests.
- For an allowed browser Origin, the gateway accepts only `session.authenticate` before binding a fully validated OIDC identity, and closes authentication failures or timeouts without admitting a room operation.
- It persists ordered immutable room events before broadcasting them and supports replay after a sequence cursor.
- Only a human participant can confirm, edit, or dismiss a draft decision; no proposal affects active context without that action.
- A deterministic gateway facilitator may derive a draft proposal only from a persisted
  `message.created` event whose trimmed text starts with `Decision:` and has a non-empty
  remainder. It attributes that proposal to the gateway agent, retains the triggering
  event identifier as provenance, and never invokes a decision transition or an external
  model, tool, or operating-system command.

## Interfaces

The gateway consumes the `n2n.room.v1` contract method `session.authenticate` for browser connection establishment and the authenticated methods `room.join`, `chat.send`, `decision.propose`, and `decision.transition`. It exposes their normalized events and errors over its WebSocket, governed by [root Decision 002](../../../../.ai/specs/decisions/002-browser-websocket-authentication.md) and the accepted [local browser profile](../decisions/002-browser-session-authentication.md).

## Explicit exclusions

This project does not own contract definitions, provide a browser UI, compose local infrastructure, grant agents database credentials or shell access, or integrate external agent runtimes, A2A, or MCP services.
