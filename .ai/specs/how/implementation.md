# Room gateway implementation

Follow the [root MVP foundation implementation plan](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/n2n-mvp-foundation-implementation-plan.md), the [ThoughtKhoral identity migration design](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/thoughtkhoral-identity-migration.md), [decision 005](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/005-room-scoped-poc-memory.md), and the root governance decisions before changing this project.

Implementation begins only after the relevant task is approved. The gateway must validate untrusted input, persist before broadcast, and retain the human approval boundary for active room context.

The accepted local [ThoughtKhoral identity decision](../decisions/002-thoughtkhoral-identity.md) renames this project to `thought-khoral-room-gateway`. The `n2n.room.v1` wire value, vendored compatibility archive, database identifiers, and persisted values remain unchanged.

## Runtime identity and configuration

The Cargo package, library crate, and executable are named
`thought-khoral-room-gateway` (with Rust's `thought_khoral_room_gateway` import
form for the library crate). `GET /health` returns `ThoughtKhoral` as the
product, `thought-khoral-room-gateway` as the service, and `ok` as the status.
Startup logs emit the same product and service labels.

New service configuration uses `THOUGHT_KHORAL_` names:
`THOUGHT_KHORAL_ALLOWED_ORIGINS`, `THOUGHT_KHORAL_LISTEN_ADDRESS`,
`THOUGHT_KHORAL_OIDC_ISSUER`, `THOUGHT_KHORAL_OIDC_AUDIENCE`,
`THOUGHT_KHORAL_OIDC_JWKS`, and
`THOUGHT_KHORAL_SESSION_AUTH_TIMEOUT_MS`. The database retains the conventional
`DATABASE_URL` name. Legacy `N2N_*` aliases are not read because a missed
deployment rename must fail at startup.

The existing `n2n_role` JWT claim is an authentication payload compatibility
field and remains unchanged alongside the `n2n.room.v1` wire values. Renaming
either requires a separately approved compatibility migration.

## Contract pin and schema-validation dependency

The gateway vendors the `thought-khoral-contracts` historical release `n2n-room-v1.0.2` beneath
`contracts/n2n.room.v1/`; it does not import the sibling repository or expose
it as a Rust crate. `contracts/lock.json` records the release commit and a
SHA-256 of the Git archive, making the input to `include_str!` reproducible.
Release `n2n-room-v1.0.2` resolves to commit
`e2e3ead757c8b35bfd330e8ea76875e9db264ac3`; its Git archive SHA-256 is
`c38cf237d873fbee62928dbccd6eba0fc5163806914ca31bba4909d95b6fbeed`.
The embedded schema SHA-256 values are `1d1490ed...2857` for envelope,
`20f00404...3cef` for RPC, and `5eaa9128...f9c5` for room event, with the full
digests recorded in `contracts/lock.json`.

The gateway uses `jsonschema` **0.42.2** with default features disabled. This
is the maintained Rust validator published by the `Stranger6667/jsonschema`
project under the MIT license. It validates the contract's JSON Schema Draft
2020-12 documents, including the UUID and RFC 3339 date-time formats, against
the pinned in-memory schemas; disabled default features prevent remote HTTP or
filesystem `$ref` resolution for untrusted requests. The selected release has
an MSRV of Rust 1.83.0, and the local toolchain is Rust 1.93.1, so it is
compatible with this gateway. The upstream documentation describes reusable
validators, the supported JSON Schema drafts, its default resolver features,
and its MIT license at
https://github.com/Stranger6667/jsonschema.

`Cargo.lock` commits the exact resolved versions. The validator-only normal
dependency graph below was generated from `cargo tree --locked --package
jsonschema --edges normal`; every version and SPDX expression was checked from
the matching downloaded crate's `Cargo.toml`. All listed licenses are
permissive and compatible with the gateway's Apache-2.0 distribution. The
`resolve-http` and `resolve-file` default features are off, so no network or
filesystem resolver dependencies are admitted by this validator selection.

| Locked packages | SPDX license expression | Compatibility conclusion |
| --- | --- | --- |
| `jsonschema 0.42.2`, `referencing 0.42.2`, `data-encoding 2.11.1`, `email_address 0.2.9`, `fancy-regex 0.17.0`, `fluent-uri 0.4.1`, `outref 0.5.2`, `uuid-simd 0.8.0`, `vsimd 0.8.0`, `synstructure 0.13.2`, `zmij 1.0.23` | MIT | Compatible permissive license. |
| `ahash 0.8.12`, `allocator-api2 0.2.21`, `bit-set 0.8.0`, `bit-vec 0.8.0`, `cfg-if 1.0.4`, `displaydoc 0.2.7`, `equivalent 1.0.2`, `fraction 0.15.4`, `getrandom 0.3.4`, `hashbrown 0.16.1`, `idna 1.1.0`, `idna_adapter 1.2.2`, `itoa 1.0.18`, `lazy_static 1.5.0`, `libc 0.2.189`, `lock_api 0.4.14`, `num 0.4.3`, `num-bigint 0.4.8`, `num-complex 0.4.6`, `num-integer 0.1.47`, `num-iter 0.1.46`, `num-rational 0.4.2`, `num-traits 0.2.19`, `once_cell 1.21.4`, `parking_lot 0.12.5`, `parking_lot_core 0.9.12`, `percent-encoding 2.3.2`, `proc-macro2 1.0.107`, `quote 1.0.47`, `ref-cast 1.0.27`, `ref-cast-impl 1.0.27`, `regex 1.13.1`, `regex-automata 0.4.18`, `regex-syntax 0.8.11`, `scopeguard 1.2.0`, `serde 1.0.229`, `serde_core 1.0.229`, `serde_derive 1.0.229`, `serde_json 1.0.151`, `smallvec 1.16.1`, `stable_deref_trait 1.2.1`, `syn 2.0.119`, `syn 3.0.5`, `utf8_iter 1.0.4` | MIT/Apache-2.0 dual license (ordering varies by crate); `aho-corasick 1.1.5` and `memchr 2.8.3` are Unlicense OR MIT | Compatible permissive licenses. |
| `borrow-or-share 0.2.4` | MIT-0 | Compatible permissive license. |
| `foldhash 0.2.0` | Zlib | Compatible permissive license. |
| `icu_collections 2.3.0`, `icu_locale_core 2.3.0`, `icu_normalizer 2.3.0`, `icu_normalizer_data 2.3.0`, `icu_properties 2.3.0`, `icu_properties_data 2.3.0`, `icu_provider 2.3.1`, `litemap 0.8.3`, `potential_utf 0.1.6`, `tinystr 0.8.4`, `writeable 0.6.4`, `yoke 0.8.3`, `yoke-derive 0.8.2`, `zerofrom 0.1.8`, `zerofrom-derive 0.1.7`, `zerotrie 0.2.5`, `zerovec 0.11.8`, `zerovec-derive 0.11.6` | Unicode-3.0 | Compatible permissive license. |
| `unicode-general-category 1.1.0` | Apache-2.0 | Compatible permissive license. |
| `unicode-ident 1.0.24` | (MIT OR Apache-2.0) AND Unicode-3.0 | Compatible permissive licenses. |
| `zerocopy 0.8.57` | BSD-2-Clause OR Apache-2.0 OR MIT | Compatible permissive licenses. |
| `bytecount 0.6.9`, `num-cmp 0.1.0` | Apache-2.0/MIT | Compatible permissive licenses. |

## Local migration verification

The Task 3 migration check uses an isolated local PostgreSQL 16 instance at
`postgres://n2n:n2n@127.0.0.1:54329/n2n`. Run it with
`DATABASE_URL=postgres://n2n:n2n@127.0.0.1:54329/n2n sqlx migrate run` from
the gateway root. These are development-only credentials for the temporary
container and are not a production configuration.

## OIDC JWT dependency decision and evidence

The gateway pins `jsonwebtoken` **11.0.0** with default features disabled and
only the `rust_crypto` feature enabled. The crate is maintained at
https://github.com/Keats/jsonwebtoken, is published under the MIT license, and
declares Rust 1.88.0 as its minimum supported version. The gateway's validated
toolchain is Rust 1.93.1, so this selection is compatible. Version 11 exposes
typed JWK/JWKS parsing, `kid` selection, reusable decoding keys, an explicit
cryptography provider, algorithm allow-listing, and validation for `exp`,
`nbf`, `aud`, `iss`, and `sub`; those capabilities cover Keycloak access-token
verification without adding an OIDC discovery or remote-key-fetch path to the
MVP. The upstream package metadata and API documentation are at
https://crates.io/crates/jsonwebtoken/11.0.0 and
https://docs.rs/jsonwebtoken/11.0.0.

The exact normal graph was resolved before adoption with
`cargo tree --edges normal` on 2026-09-11. `jsonwebtoken 11.0.0` resolves its
cryptographic path through `signature 2.2.0`, `rsa 0.9.10`, `sha2 0.10.9`,
`hmac 0.12.1`, `p256 0.13.2`, `p384 0.13.1`, `ed25519-dalek 2.2.0`,
`curve25519-dalek 4.1.3`, and their RustCrypto support crates. Package metadata
reports MIT, Apache-2.0 OR MIT, MIT/Apache-2.0, BSD-1-Clause, BSD-2-Clause,
BSD-3-Clause, Unicode-3.0, or Unlicense OR MIT expressions throughout the
selected target graph. Every expression is permissive and compatible with this
Apache-2.0 project; no GPL, LGPL, AGPL, SSPL, proprietary, or unknown-license
package is admitted. `Cargo.lock` records the final exact resolution used by
this repository.

`rust_crypto` was selected instead of `aws_lc_rs` because the MVP needs only
portable signature verification and does not need a native AWS-LC build. The
feature also supports test-only RSA signing without weakening the production
algorithm policy. Default `use_pem` is disabled, so production accepts public
JWK material rather than private or PEM key configuration.

## MVP authentication and WebSocket security decisions

Decision [001](../decisions/001-runtime-security-and-publication.md) is
accepted as of 2026-09-11T19:40:03Z and records the maturity, compatibility,
and complete locked transitive-license audit for Axum, tracing,
tracing-subscriber, tokio-tungstenite, RSA, rand, and futures-util. Dependency
or feature changes require reapproval of that audit.

Root [Decision 002](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/002-browser-websocket-authentication.md)
and the accepted local [browser authentication profile](../decisions/002-browser-session-authentication.md)
govern connection authentication. An Origin-bearing browser upgrade must
exactly match the required configured allowlist, upgrades without binding an
HTTP bearer identity, and accepts only `session.authenticate` within the
bounded timeout. A no-Origin non-browser agent retains the existing mandatory
Bearer upgrade path. Both paths use the same validator and token-expiration
close behavior; neither logs credentials or raw room content.

- Startup requires an OIDC issuer, audience, and a configured JWKS JSON
  document. The MVP deliberately does not fetch discovery or JWKS URLs at
  request time; key rotation is an explicit configuration rollout, eliminating
  SSRF and unbounded remote-fetch behavior from the authenticated boundary.
- Access tokens are accepted from browser `session.authenticate` messages or,
  for no-Origin non-browser clients, the HTTP `Authorization: Bearer` header;
  never a URL query parameter. Validation requires a `kid`, an exact matching
  signature-use JWK, RS256, a valid signature, unexpired `exp`, exact `iss`, an
  allowed `aud`, a non-empty UUID `sub`, and `n2n_role` equal to exactly
  `human` or `agent`. Missing, malformed, expired, mismatched, or unsupported
  claims all map to `-32001`; no claim is defaulted. JWT clock leeway is zero.
  At validation time `now`, acceptance requires `exp > now` and optional
  `nbf <= now`: `exp == now` is rejected and `nbf == now` is accepted.
- A no-Origin client without a valid bearer token is rejected before upgrade
  with HTTP 401 and `-32001`. An allowed-origin browser is briefly upgraded
  unauthenticated, restricted to `session.authenticate`, and closed on failure
  or timeout. An authenticated socket must successfully send `room.join`
  before room mutations; that join binds it to one room, and cross-room
  requests are forbidden.
- Agents may join, chat, and propose, but only a human may invoke
  `decision.transition`; an agent attempt returns `-32003` without a database
  write or broadcast. Draft-only transition rules are checked while the
  decision row is locked. An edit supersedes the draft, creates its active
  replacement, persists both immutable events, and records idempotency in the
  same transaction.
- The canonical validated request parameters are stored as JSONB for
  idempotency. Reusing `(roomId, requestId)` with the same canonical request
  returns the original persisted event or events without rebroadcasting;
  different parameters return `-32012`.
- Accepted events are committed before publication to the room broadcast
  channel. Replays read committed rows where `sequence > afterSequence` in
  ascending order before the connection consumes live room events. Live
  delivery never advances over a sequence gap: it replays from the last
  delivered cursor first, then suppresses any later queued duplicate.
- Rejection logs contain only error code plus available request, room, actor,
  and event identifiers. Raw JWTs, untrusted message text, titles, summaries,
  complete request bodies, and stack traces are never logged.

## Facilitator draft-proposal port

The gateway owns the facilitator **port**. After a room event is persisted, the
gateway asks that port for zero or more drafts, records any result as
`decision.proposed` in the same authorization and persistence boundary, and
never lets that path invoke `decision.transition`. Human-only
`decision.transition` remains the sole path to active context.

The live implementation is the deterministic `Decision:` parser. After a
`chat.send` has been normalized and persisted as `message.created`, the
gateway may inspect that persisted event locally. Only trimmed text beginning
with `Decision:` and followed by a non-empty title produces a second persisted
event, `decision.proposed`. The proposal is a draft attributed to the fixed
gateway agent, cites exactly the triggering message event ID in
`sourceEventIds`, and is published only after the transaction commits. This
implementation makes no model, tool, operating-system, or non-gateway-database
call.

A later Cognee / memory-engine implementation may occupy the same port under
root [decision 005](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/005-room-scoped-poc-memory.md).
This project must not embed Cognee, call an unmediated model, or add a second
independent propose path beside the port. Switching the live implementation
requires a separately approved memory-engine How and implementation plan.

## Append-only and persistence validation invariants

`room_events` is database-enforced append-only: a follow-on migration installs
a trigger that rejects every `UPDATE` and `DELETE` using SQLSTATE `55000`.
This prevents sequence reuse after a highest event is removed, even if a
future application path bypasses `append_event`. The gateway's only normal
write path is a single transaction that takes a room-scoped advisory lock,
calculates the next sequence, and inserts one event.

Before opening that transaction, `append_event` turns `NewEvent` into the
normalized `n2n.room.v1` persisted-event shape and validates it against the
pinned `room-event.schema.json`, with the same in-memory Draft 2020-12
validator and UUID/date-time format checks as the request boundary. Invalid
event type, actor role, or non-object payload therefore returns `StoreError`
without creating a row. The migration retains the room/event uniqueness
constraints as a second database integrity boundary.
