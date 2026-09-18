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

## Consequences

- `contracts/n2n.room.v1/` and `contracts/lock.json` must identify the
  extension snapshot used by the gateway.
- `src/protocol.rs` validates the new method/event against the vendored
  snapshot.
- Messages beginning with `Decision:` remain ordinary chat.
- Existing room events and audit history remain append-only.
