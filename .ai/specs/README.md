# ThoughtKhoral room gateway specifications

Parent requirements in the ThoughtKhoral root `.ai/specs/` apply here. This project may diverge only through an accepted local decision record that identifies the overridden parent rule and its consequences.

See the [root specification index](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/README.md).

## Local areas

- [What: MVP room gateway](what/mvp-room.md)
- [What: public documentation](what/public-documentation.md)
- [How: implementation](how/implementation.md)
- [Decisions](decisions/README.md)
- [Decision 002: ThoughtKhoral project identity](decisions/002-thoughtkhoral-identity.md)

## Facilitator draft-proposal port

The gateway owns the facilitator port, but the legacy `Decision:` prefix
parser is not active. A human uses `/decisions` for governed mutations;
Cognee may occupy the derived-draft port later from
`thought-khoral-memory-engine` and is not a runtime of this project. See root
[decision 005](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/005-room-scoped-poc-memory.md),
[decision 008](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/008-slash-decisions-and-facilitator-boundary.md),
and the [memory-engine POC What](https://github.com/thoughtkhoral/thought-khoral-memory-engine/blob/main/.ai/specs/what/poc-room-scoped-memory.md).

## Room lifecycle compatibility boundary

The workspace UI owns explicit Enter/Leave presentation and closes its client
socket on Leave. The platform owns the non-joining browser bootstrap and
non-secret room URL binding. This gateway and the retained v1 contract remain
unchanged for that lifecycle work: there is no `room.leave` RPC, durable room
membership, kick operation, or history deletion. See the [workspace UI
lifecycle specification](https://github.com/thoughtkhoral/thought-khoral-workspace-ui/blob/main/.ai/specs/what/mvp-ui.md)
and [platform lifecycle specification](https://github.com/thoughtkhoral/thought-khoral-platform/blob/main/.ai/specs/what/local-mvp.md).
