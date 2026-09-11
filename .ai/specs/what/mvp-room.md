# MVP room gateway

## Sole MVP responsibility

`n2n-room-gateway` is the sole mediator that validates, persists, replays, and broadcasts governed room events and active-context updates.

## Acceptance criteria

- The gateway validates supported `n2n.room.v1` JSON-RPC requests and returns the specified structured errors for rejected requests.
- It persists ordered immutable room events before broadcasting them and supports replay after a sequence cursor.
- Only a human participant can confirm, edit, or dismiss a draft decision; no proposal affects active context without that action.

## Interfaces

The gateway consumes the `n2n.room.v1` contract methods `room.join`, `chat.send`, `decision.propose`, and `decision.transition`, and exposes their normalized events and errors over its authenticated room WebSocket.

## Explicit exclusions

This project does not own contract definitions, provide a browser UI, compose local infrastructure, grant agents database credentials or shell access, or integrate external agent runtimes, A2A, or MCP services.
