# Task 9 gateway fix report

Base commit: `5ebe4a5`

## Follow-up fixes

- Replaced the Clippy-obfuscated fallback expressions in the room mention-token helpers.
- Persisted `message.created` validation now rejects duplicate semantic mention identities: a participant UUID may appear once regardless of token, and an alias may appear once.
- Added store and validator regressions for duplicate participant IDs with different tokens and duplicate aliases.
- Kept request duplicate-ID coverage isolated to the same canonical token twice.
- Added a reverse-order live-recovery regression proving a hidden targeted event advances the cursor without reaching an unauthorized observer, while a later public event still arrives.
- Extended the vendored contract fixture harness with an invalid persisted-event duplicate-alias fixture and documented the semantic uniqueness rule.

## Final contract synchronization

- Replaced the complete `contracts/n2n.room.v1` tracked artifact set from authoritative `thought-khoral-contracts` commit `3ca4d324a48f3f7a9158d76da959ccd799596036`, including schemas, protocol, package metadata and lockfile, fixture harness, and all valid/invalid request and persisted-event fixtures.
- Migrated the former gateway-only `fixtures/invalid-events` location to the source canonical `fixtures/events/invalid` layout; no content-level gateway vendor differences remain. The only intentional difference is the necessary gateway vendor prefix `contracts/n2n.room.v1/`.
- Updated `contracts/lock.json` provenance to the source commit, its `git archive` SHA-256, and the three authoritative schema SHA-256 values. The source commit has no containing release tag, so `tag` is explicitly `null`.

## Final verification

- Exact committed-tree comparison: 37 authoritative files equal 37 vendored files after excluding ignored `node_modules`; the gateway vendor prefix is the sole path difference.
- `npm test` from `contracts/n2n.room.v1` passed, including request and persisted-event fixture validation.
- With `DATABASE_URL=postgres://n2n:n2n@127.0.0.1:54329/n2n`: `cargo test` passed (78 tests); `cargo fmt --check`, `cargo check`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check` passed.

## Verification

Run with `DATABASE_URL=postgres://n2n:n2n@127.0.0.1:54329/n2n`:

- focused protocol, store, request, replay, and contract fixture tests
- `cargo test`
- `cargo fmt --check`
- `cargo check`
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
