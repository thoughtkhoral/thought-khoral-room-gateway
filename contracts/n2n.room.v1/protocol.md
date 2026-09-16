# N:N room protocol v1

`n2n.room.v1` defines JSON-RPC 2.0 room requests and normalized, immutable room events. Every request has `jsonrpc: "2.0"`, a non-empty string `id`, a supported `method`, and object `params`. The connection-establishment request `session.authenticate` has only a non-empty `accessToken`; authenticated room-operation payloads include `contractVersion: "n2n.room.v1"`, UUID `requestId`, UUID `roomId`, and RFC 3339 `occurredAt`.

## Methods

| Method | Parameters | Success result |
| --- | --- | --- |
| `session.authenticate` | `accessToken` | Authenticated connection identity and role. |
| `room.join` | `roomId`, `requestId`, `occurredAt`, `afterSequence?` | Ordered room snapshot, events after the cursor, and the participant snapshot. |
| `chat.send` | `roomId`, `requestId`, `occurredAt`, `text` | Normalized `message.created` event. |
| `decision.propose` | `roomId`, `requestId`, `occurredAt`, `title`, `summary`, `sourceEventIds` | `decision.proposed` event. |
| `decision.transition` | `roomId`, `requestId`, `occurredAt`, `decisionId`, `action`, `editedTitle?`, `editedSummary?` | `decision.confirmed`, `decision.edited`, or `decision.dismissed` event. |

Only `confirm`, `edit`, and `dismiss` are valid actions. `edit` requires non-empty `editedTitle` and `editedSummary`; the other actions must not supply either edit field.

The gateway may send the JSON-RPC notification `room.participants.updated` without
an `id`. Its params contain `contractVersion`, `roomId`, and a `participants`
array. Each participant has `id`, `role`, `displayName`, and `online`. The list
contains actors found in room history and currently joined connections; presence
is ephemeral and is not persisted as a room event.

## Browser WebSocket authentication

Per the accepted root [browser WebSocket authentication decision](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/002-browser-websocket-authentication.md), a browser opens an unauthenticated connection and its first application message must be `session.authenticate`. Before token validation succeeds, the gateway permits no method other than `session.authenticate`; it validates that access token's issuer, audience, signature, key identifier, algorithm, expiry, and not-before claims before binding identity and role to the connection. The schema validates only that the token is a non-empty string and must not cause a token to be logged or retained.

Failed authentication produces structured error `-32001` and the gateway closes the connection when it can send the error. A connection that does not authenticate within its short configured timeout is closed. These failed-authentication and timeout rules are normative gateway behavior, not client fallback behavior.

## Errors

| Code | Meaning |
| --- | --- |
| `-32600` | Invalid request. |
| `-32601` | Unknown method. |
| `-32001` | Unauthenticated. |
| `-32003` | Forbidden. |
| `-32004` | Room or decision not found. |
| `-32009` | Unsupported contract version. |
| `-32010` | Invalid state transition. |
| `-32011` | Expired context packet. |
| `-32012` | Duplicate request with a different payload. |

## Decision transitions

Only a participant with the `human` role may invoke `decision.transition`. `confirm` changes `draft` to `active`; `dismiss` changes `draft` to `dismissed`; `edit` changes the old draft to `superseded`, creates a new `active` decision with `derivedFromDecisionId` set to the old identifier, and emits both immutable events in one database transaction.

## Compatibility

This accepted addition of the pre-authentication `session.authenticate` handshake is an additive `n2n.room.v1` patch and preserves all pre-existing authenticated room methods. Other additive optional fields are minor-compatible. Required-field, enum, method, or semantic changes require a new major contract version.
