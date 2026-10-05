# Codex Room Participation

## Status

Approved by the project maintainer in the Codex working session on 2026-10-05,
including this milestone-one specification and the coordinated implementation
plan. Accepted contribution: [issue 1](https://github.com/thoughtkhoral/thought-khoral-room-gateway/issues/1).
Implementation follows the [plan](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/codex-room-conversations-implementation-plan.md) and its dependency gates.
Release/tag publication, provider use and service activation require their
separate later authorization. No completed runtime or live verification is claimed.

## Governing sources

- [Root Codex room requirements](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/what/codex-chat-agent.md)
- [Root Decision 009](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/009-codex-chat-agent.md)
- [Contracts profile](https://github.com/thoughtkhoral/thought-khoral-contracts/blob/thought-khoral-agent-conversation-v1.0.0/.ai/specs/how/agent-conversation-profile.md)

## Responsibility

Own authenticated shared room conversations, atomic prompt/task reservation,
authorized baseline/delta context, profile task lifecycle, result-to-room-message
persistence, and recovery. One active generation exists per room/admitted agent.
Keep runtime thread IDs, provider credentials, and Codex subprocesses outside this
repository. Ordinary retained chat.send keeps its existing trigger behavior.

## Acceptance criteria

- Explicit human addressing produces exactly one authorized task and public reply;
  ordinary messages, aliases, quoted mentions, and agent replies do not invoke it.
- Full authorized room-wide baseline and later ordered deltas preserve discussion
  across multiple humans, worker restart, hidden sequence gaps, and browser reload.
- Targeted content, another room, wrong generation/base/digest, stale authority,
  unknown admission, and late/duplicate conflicting terminal results are rejected.
- Fresh native threads receive authorized room history again; reset cannot run
  while busy and never silently restores the previous thread after failure.
- Supported controls and unavailable capabilities follow the exact profile;
  model denial never silently substitutes a model and usage is last-request based.
- Retained room fixtures and deterministic Reference Agent regression gates pass.
- Repository-specific deliverables/tests in the [implementation plan](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/codex-room-conversations-implementation-plan.md)
  pass before its milestone is accepted; live coverage is identified separately.

## Exclusions

Directory guidance/curated-memory bundles are milestone two. Autonomous turns,
private/shared-audience exceptions, host repository editing, production admission,
and arbitrary external agents remain outside milestone one.
