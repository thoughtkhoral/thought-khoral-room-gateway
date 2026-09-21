# ThoughtKhoral room protocol v1

`n2n.room.v1` defines JSON-RPC 2.0 room requests and normalized, immutable room events. Every request has `jsonrpc: "2.0"`, a non-empty string `id`, a supported `method`, and object `params`. The connection-establishment request `session.authenticate` has only a non-empty `accessToken`; authenticated room-operation payloads include `contractVersion: "n2n.room.v1"`, UUID `requestId`, UUID `roomId`, and RFC 3339 `occurredAt`.

## Methods

| Method | Parameters | Success result |
| --- | --- | --- |
| `session.authenticate` | `accessToken` | Authenticated connection identity and role. |
| `room.join` | `roomId`, `requestId`, `occurredAt`, `afterSequence?` | Ordered room snapshot, events after the cursor, and the participant snapshot. |
| `chat.send` | `roomId`, `requestId`, `occurredAt`, `text`, `mentions?`, `delivery?` | Normalized `message.created` event. |
| `decision.propose` | `roomId`, `requestId`, `occurredAt`, `title`, `summary`, `sourceEventIds` | `decision.proposed` event. |
| `decision.transition` | `roomId`, `requestId`, `occurredAt`, `decisionId`, `action`, `editedTitle?`, `editedSummary?` | `decision.confirmed`, `decision.edited`, or `decision.dismissed` event. |
| `decision.delete` | `roomId`, `requestId`, `occurredAt`, `decisionId` | `decision.deleted` event. |
| `agent.task.start` | `roomId`, `requestId`, `occurredAt`, `agentId`, `skillId`, `input` | Accepted external-agent task. |

Only `confirm`, `edit`, and `dismiss` are valid actions. `edit` requires non-empty `editedTitle` and `editedSummary`; the other actions must not supply either edit field.

`decision.propose` accepts an empty `sourceEventIds` array when a decision has no source evidence.

`chat.send` remains room-wide when `delivery` is omitted or set to `room`. For targeted delivery, set `delivery` to `mentioned` and provide one or more `mentions`, up to 50 targets. A participant target has `type: "participant"`, a UUID `id`, and a lower-case ASCII slug `token` matching the direct mention token; both single-word tokens such as `maya` and hyphen-separated tokens such as `maya-chen` are valid. An alias target has `type: "alias"` and is restricted to the fixed aliases `allhumans` and `allagents`. Mention identities are semantically unique: participant targets are unique by `id`, and alias targets are unique by `alias`, even if their other fields differ. This rule applies to both `chat.send` requests and persisted `message.created` payloads; JSON Schema `uniqueItems` rejects only structurally identical items, so contract validation enforces semantic uniqueness. The request schema enforces this target shape and token boundary; resolving whether a participant is currently addressable is gateway behavior.

The `@allhumans` alias targets all known human participants; agents are not included unless independently mentioned. The `@allagents` alias targets all known agents and is also visible to all known human participants in the room. A targeted message is replayed only to the resolved audience, while room-wide messages are replayed to all room participants. The gateway returns `-32013` when a direct participant target cannot be resolved or its token is not canonical for the current roster.

## Governed external-agent tasks

`agent.task.start` begins a governed external-agent task. Its `skillId` is
restricted to `summarize-context` or `extract-action-items`, and `input` is a
non-empty string of at most 8,000 characters. The gateway records the
additive task-event order `agent.task.requested`, zero or more
`agent.task.progressed`, optionally `agent.task.awaiting_external_input`, and
one terminal `agent.task.succeeded` or `agent.task.failed`. Every new event
has `taskId`, `agentId`, `requesterId`, `skillId`, and `contextRevision`.

Progress is durable only when the task reaches a meaningful change of phase:
`accepted`, `retrieving-context`, `working`, or `finalizing`. Its `text` is
bounded to 512 characters and optional `percent` is an integer from 0 through
100. A terminal event is immutable and never coalesced with another terminal
event.

Successful external tasks use a bounded discriminated `result`: either
`context-summary.v1` with `summary` and `citations`, or `action-items.v1` with
`actionItems` and `citations`. Citation values are UUID names for sources
visible in the task's context packet; the gateway verifies packet visibility
before it persists the event. A failure carries only the safe `failure.code`
(`invalid_task_input` or `execution_failed`).

An `agent.task.awaiting_external_input` event has only the task core and a
handoff containing an instruction, HTTPS URL, host, and expiry. Browser and
agent secrets, tokens, credentials, request headers, and callback bodies never
enter this contract. The handoff is an instruction to a user or external
system, not an authorization channel.

The existing retained `agent.task.queued`, `agent.task.running`, and
`agent.task.succeeded` Action Items fixtures remain valid legacy v1 events;
the governed external-agent shapes are additive and do not reinterpret them.

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
| `-32013` | Mention target not found. |

## Decision transitions

Only a participant with the `human` role may invoke `decision.transition`. `confirm` changes `draft` to `active`; `dismiss` changes `draft` to `dismissed`; `edit` changes the old draft to `superseded`, creates a new `active` decision with `derivedFromDecisionId` set to the old identifier, and emits both immutable events in one database transaction.

Only a participant with the `human` role may invoke `decision.delete`. The gateway locks the requested decision row, copies its `decisionId`, prior status, title, summary, and source-event IDs into a `decision.deleted` audit event, deletes the row, and commits the deletion, event, and request-ledger record atomically. A missing decision returns `-32004`; a non-human caller returns `-32003`. `dismiss` remains a state transition and is not the physical delete operation.

## Compatibility

`n2n.room.v1` remains a retained compatibility wire value for the ThoughtKhoral project. Existing room requests and events remain compatible, and the deletion method/event and empty-source create rule are additive within this retained contract. The optional event actor `displayName`, participant snapshot, and participant notification are additive fields/messages and do not invalidate existing events or room requests.

This accepted addition of the pre-authentication `session.authenticate` handshake is an additive `n2n.room.v1` patch and preserves all pre-existing authenticated room methods. Other additive optional fields are minor-compatible. Required-field, enum, method, or semantic changes require a new major contract version. A future `thought-khoral.room.v2` protocol is a separate migration and requires its own approved compatibility decision.
