# N:N room protocol v1

`n2n.room.v1` defines JSON-RPC 2.0 room requests and normalized, immutable room events. Every request has `jsonrpc: "2.0"`, a non-empty string `id`, a supported `method`, and object `params`. Every application payload includes `contractVersion: "n2n.room.v1"`, UUID `requestId`, UUID `roomId`, and RFC 3339 `occurredAt`.

## Methods

| Method | Parameters | Success result |
| --- | --- | --- |
| `room.join` | `roomId`, `requestId`, `occurredAt`, `afterSequence?` | Ordered room snapshot and events after the cursor. |
| `chat.send` | `roomId`, `requestId`, `occurredAt`, `text` | Normalized `message.created` event. |
| `decision.propose` | `roomId`, `requestId`, `occurredAt`, `title`, `summary`, `sourceEventIds` | `decision.proposed` event. |
| `decision.transition` | `roomId`, `requestId`, `occurredAt`, `decisionId`, `action`, `editedTitle?`, `editedSummary?` | `decision.confirmed`, `decision.edited`, or `decision.dismissed` event. |

Only `confirm`, `edit`, and `dismiss` are valid actions. `edit` requires non-empty `editedTitle` and `editedSummary`; the other actions must not supply either edit field.

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

## WebSocket authentication failure

An unauthenticated WebSocket upgrade is rejected with the structured JSON-RPC error code `-32001`; if a WebSocket session has already been established and authentication becomes invalid, the gateway sends that error when possible and closes the connection. Authentication failures never grant room access or emit room events.

## Compatibility

Additive optional fields are minor-compatible. Required-field, enum, method, or semantic changes require a new major contract version.
