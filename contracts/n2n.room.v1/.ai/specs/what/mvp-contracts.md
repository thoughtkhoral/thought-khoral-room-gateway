# MVP contracts

## Sole MVP responsibility

`n2n-contracts` is the compatibility authority for the versioned, language-neutral `n2n.room.v1` JSON Schema artifacts, normative protocol documentation, and compatibility fixtures.

## Acceptance criteria

- Schemas define the `n2n.room.v1` envelopes, JSON-RPC requests, and room events.
- Valid and invalid fixtures prove the contract validator accepts and rejects the specified payloads.
- Protocol documentation records method semantics, structured errors, decision transitions, and compatibility rules.

## Interfaces

The project publishes `n2n.room.v1` for `room.join`, `chat.send`, `decision.propose`, and `decision.transition`, including their JSON-RPC 2.0 envelopes, `contractVersion`, RFC 4122 `requestId`, and structured error codes.

## Explicit exclusions

This project does not provide a shared runtime library, gateway implementation, user interface, persistence service, authentication provider, or platform deployment.
