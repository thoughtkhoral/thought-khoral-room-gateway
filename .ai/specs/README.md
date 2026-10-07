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

## Approved Codex room-participation extension

- [What: codex room participation](what/codex-room-participation.md)
- [How: codex room participation](how/codex-room-participation.md)
- [Coordinated implementation plan](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/codex-room-conversations-implementation-plan.md)

Approved by the maintainer on 2026-10-05 under [issue 1](https://github.com/thoughtkhoral/thought-khoral-room-gateway/issues/1).
Implementation follows the coordinated plan and its artifact/dependency gates.
Existing runtime behavior is unchanged until the relevant tasks pass verification.

## Task 9 synthetic verification and correction checkpoint — 2026-10-07

Local corrections and synthetic evidence are recorded in the [owning checkpoint](how/codex-room-participation.md). The approved defaults amendment and its correction for UI finding F1 are
accepted on reviewed local synthetic candidate branches. Contract publication, packaged-stack and separately authorized provider/live evidence remain pending. Original runtime is retained; local corrected branches are unmerged.

## Approved defaults-discovery amendment — 2026-10-07

The [approved design](https://github.com/thoughtkhoral/thought-khoral-codex-agent/blob/main/.ai/specs/how/default-settings-discovery-proposal.md) authorizes local defaults discovery and
independent optional controls, with verified unreleased candidate contract pins.
Implementation and synthetic verification follow the amendment plan; publication,
provider use, activation, merge and push retain their separate gates.
