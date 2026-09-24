# 003 — Slash decisions CRUD contract extension

## Status

Accepted

## Decision

This repository vendors the additive `n2n.room.v1` slash-decisions extension
from the contracts repository. The extension adds `decision.delete` and
`decision.deleted`, allows empty `sourceEventIds` on `decision.propose`, and
removes the legacy `Decision:` message-prefix proposal side effect.

The existing `n2n-room-v1.0.2` archive remains immutable. This accepted local
decision authorizes the next vendored compatibility snapshot to carry the
additive extension without introducing a new protocol major version. The
gateway remains the authority for human-only deletion, audit-event persistence,
and physical decision-row deletion.

## Parent requirement superseded

Root [decision 005](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/005-room-scoped-poc-memory.md)
required the deterministic `Decision:` prefix parser to remain the live
facilitator implementation until a memory-engine switch. Root
[decision 008](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/008-slash-decisions-and-facilitator-boundary.md)
supersedes that specific requirement. The replacement rule is that
`Decision:` is ordinary chat, while human `/decisions` actions use governed
decision RPCs. The gateway-owned facilitator port remains the sole path for
future drafts derived from persisted room events; no automatic prefix parser
is active. Room partitioning and human activation authority from decision 005
remain in force.

## Consequences

- `contracts/n2n.room.v1/` and `contracts/lock.json` must identify the
  extension snapshot used by the gateway.
- `src/protocol.rs` validates the new method/event against the vendored
  snapshot.
- Messages beginning with `Decision:` remain ordinary chat.
- Existing room events and audit history remain append-only.
- Future memory-derived proposals still require the facilitator port and a
  separately approved activation plan; human `/decisions` creation is not a
  second derived-draft proposer.
