# 002 — ThoughtKhoral project identity

## Status

Accepted

## Decision

This repository implements root [Decision 003 — ThoughtKhoral product identity](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/003-thoughtkhoral-product-identity.md). Its direct-child directory is renamed exactly from `n2n-contracts` to `thought-khoral-contracts`.

The existing `n2n.room.v1` protocol value, schema `$id` and `$ref` identifiers,
`contractVersion` constants, fixture payloads, and immutable release history
remain wire-compatible and unchanged. Human-facing schema `title` metadata is
product display text and uses ThoughtKhoral in the live contract; changing that
metadata does not change a v1 wire value. Database identifiers, database
contents, persisted records, persisted fields, and persisted values are
excluded from this rename.

## Consequences

- New project-facing identifiers use `thought-khoral-contracts`.
- A future protocol or data rename requires its own approved compatibility and migration decision.
