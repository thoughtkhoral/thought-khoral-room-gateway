# Task 9 gateway fix report

Base commit: `5ebe4a5`

## Follow-up fixes

- Replaced the Clippy-obfuscated fallback expressions in the room mention-token helpers.
- Persisted `message.created` validation now rejects duplicate semantic mention identities: a participant UUID may appear once regardless of token, and an alias may appear once.
- Added store and validator regressions for duplicate participant IDs with different tokens and duplicate aliases.
- Kept request duplicate-ID coverage isolated to the same canonical token twice.
- Added a reverse-order live-recovery regression proving a hidden targeted event advances the cursor without reaching an unauthorized observer, while a later public event still arrives.
- Extended the vendored contract fixture harness with an invalid persisted-event duplicate-alias fixture and documented the semantic uniqueness rule.

## Verification

Run with `DATABASE_URL=postgres://n2n:n2n@127.0.0.1:54329/n2n`:

- focused protocol, store, request, replay, and contract fixture tests
- `cargo test`
- `cargo fmt --check`
- `cargo check`
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
