# 002 — ThoughtKhoral project identity

## Status

Accepted

## Decision

This repository implements root [Decision 003 — ThoughtKhoral product identity](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/003-thoughtkhoral-product-identity.md). Its direct-child directory is renamed exactly from `n2n-room-gateway` to `thought-khoral-room-gateway`.

The existing `n2n.room.v1` protocol values, `n2n_role` authentication claim,
and vendored compatibility archive remain wire-compatible and unchanged.
Database identifiers, database contents, persisted records, persisted fields,
and persisted values are excluded from this rename.

## Consequences

- New project-facing identifiers use `thought-khoral-room-gateway`.
- The Cargo package, library crate, and executable use
  `thought-khoral-room-gateway`; Rust imports use
  `thought_khoral_room_gateway`.
- `GET /health` and structured startup logs identify the product as
  `ThoughtKhoral` and the service as `thought-khoral-room-gateway`.
- New service configuration uses the `THOUGHT_KHORAL_` namespace. Legacy
  `N2N_*` aliases are not accepted.
- A future protocol or data rename requires its own approved compatibility and migration decision.
