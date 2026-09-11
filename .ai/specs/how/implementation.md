# Room gateway implementation

Follow the [root N:N MVP foundation implementation plan](../../../../.ai/specs/how/n2n-mvp-foundation-implementation-plan.md) and the root governance decision before changing this project.

Implementation begins only after the relevant task is approved. The gateway must validate untrusted input, persist before broadcast, and retain the human approval boundary for active room context.

## Contract pin and schema-validation dependency

The gateway vendors the `n2n-contracts` release `n2n-room-v1.0.1` beneath
`contracts/n2n.room.v1/`; it does not import the sibling repository or expose
it as a Rust crate. `contracts/lock.json` records the release commit and a
SHA-256 of the Git archive, making the input to `include_str!` reproducible.

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
