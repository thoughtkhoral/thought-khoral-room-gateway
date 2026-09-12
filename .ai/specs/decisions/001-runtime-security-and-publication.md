# 001 — Runtime dependency, JWT clock, and ordered publication policy

- **Status:** Accepted
- **Approved at:** 2026-09-11T19:40:03Z
- **Boundary amendment approved at:** 2026-09-11T20:07:23Z
- **Approval basis:** Task 4 fix-round directive from the authorized implementation controller
- **Scope:** `thought-khoral-room-gateway` MVP only
- **Parent override:** None. This decision tightens the parent requirements for authenticated, ordered room delivery.

## Context

Task 4 adopted an HTTP/WebSocket stack, structured logging, cryptographic JWT
verification, and test-only WebSocket/RSA utilities. Review required explicit
maturity, compatibility, and transitive-license evidence for every adopted
component. Review also found that a database commit order does not itself
guarantee the order in which independently scheduled handler tasks publish to
a Tokio broadcast channel, and that a 30-second JWT validation leeway admits
already-expired and not-yet-valid tokens.

## Dependency decision and authoritative evidence

Versions and dependency edges are fixed by `Cargo.lock`. The following data
comes from the locked crates' published `Cargo.toml` metadata and official
project repositories. Compatibility was also compiled on the project's Rust
1.93.1 toolchain.

| Adopted component | Locked version / license / declared MSRV | Maturity and compatibility conclusion |
| --- | --- | --- |
| Axum | `0.8.9`; MIT; Rust 1.80 | Stable 0.8 release from the official Tokio project, built on Tokio, Hyper, Tower, and Tungstenite. Its `ws` feature and Rust requirement are compatible. Source: https://github.com/tokio-rs/axum and https://docs.rs/axum/0.8.9. |
| tracing | `0.1.44`; MIT; Rust 1.65 | Established stable 0.1 instrumentation API from Tokio; compatible. Source: https://github.com/tokio-rs/tracing and https://docs.rs/tracing/0.1.44. |
| tracing-subscriber | `0.3.23`; MIT; Rust 1.65 | Established stable 0.3 collector/formatter from Tokio; compatible. Source: https://github.com/tokio-rs/tracing and https://docs.rs/tracing-subscriber/0.3.23. |
| tokio-tungstenite | `0.29.0`; MIT; Rust 1.63 | Maintained Tokio adapter from the official Tungstenite project. Version 0.29 is the line selected by Axum 0.8.9, avoiding two runtime WebSocket versions; compatible. Source: https://github.com/snapview/tokio-tungstenite and https://docs.rs/tokio-tungstenite/0.29.0. |
| RSA | `0.9.10`; MIT OR Apache-2.0; Rust 1.65 | Maintained RustCrypto implementation. Direct use is restricted to generating ephemeral 2048-bit integration-test keys; production verification is mediated by `jsonwebtoken`'s RustCrypto provider. Compatible. Source: https://github.com/RustCrypto/RSA and https://docs.rs/rsa/0.9.10. |
| rand | `0.8.8`; MIT OR Apache-2.0; no MSRV declared in published metadata | Stable 0.8 line from the official rust-random project. Direct use is test-only for ephemeral RSA generation; the exact version compiles on Rust 1.93.1. Source: https://github.com/rust-random/rand and https://docs.rs/rand/0.8.8. |
| futures-util | `0.3.34`; MIT OR Apache-2.0; Rust 1.71 | Stable 0.3 utility line from the official Rust futures project. Direct use is test-only for WebSocket stream/sink operations; compatible. Source: https://github.com/rust-lang/futures-rs and https://docs.rs/futures-util/0.3.34. |

### Locked transitive-license audit

The audit used, for each component, `cargo tree --locked --offline -p
<name>@<version> --edges normal` to select the exact closure and `cargo
metadata --format-version 1 --locked` to read every selected package's SPDX
expression. The counts and complete expression sets below are closure-specific;
exact package names and versions remain mechanically reproducible from
`Cargo.lock` with those commands.

| Root closure | Locked packages | SPDX expressions found across every node |
| --- | ---: | --- |
| `axum 0.8.9` | 77 | MIT; MIT OR Apache-2.0; Apache-2.0 OR MIT; MIT/Apache-2.0; Apache-2.0; Apache-2.0 OR BSL-1.0; MIT AND BSD-3-Clause; BSD-3-Clause; BSD-2-Clause OR Apache-2.0 OR MIT; Unlicense OR MIT; (MIT OR Apache-2.0) AND Unicode-3.0 |
| `tracing 0.1.44` | 10 | MIT; MIT OR Apache-2.0; Apache-2.0 OR MIT; (MIT OR Apache-2.0) AND Unicode-3.0 |
| `tracing-subscriber 0.3.23` | 19 | MIT; MIT OR Apache-2.0; (MIT OR Apache-2.0) AND Unicode-3.0 |
| `tokio-tungstenite 0.29.0` | 46 | MIT; MIT OR Apache-2.0; Apache-2.0 OR MIT; BSD-3-Clause; BSD-2-Clause OR Apache-2.0 OR MIT; Unlicense OR MIT; (MIT OR Apache-2.0) AND Unicode-3.0 |
| `rsa 0.9.10` | 41 | MIT; MIT OR Apache-2.0; Apache-2.0 OR MIT; MIT/Apache-2.0; BSD-3-Clause; BSD-2-Clause OR Apache-2.0 OR MIT; (MIT OR Apache-2.0) AND Unicode-3.0 |
| `rand 0.8.8` | 8 | MIT OR Apache-2.0; BSD-2-Clause OR Apache-2.0 OR MIT |
| `futures-util 0.3.34` | 8 | MIT; MIT OR Apache-2.0; Apache-2.0 OR MIT; Unlicense OR MIT |

For exact audit reproducibility, SHA-256 over each command's unmodified tree
output is: Axum `7c786957299d615102d02b00eba9f61d37e79fe615a972d4d6af03def04ce6fd`;
tracing `17c141f695d35f2b3f64b8d0afb09f1b0c9c66eac54570cd0ed2a76f436e1d2c`;
tracing-subscriber
`7053cb929789a9e17dc0a51312dcfa709f57e372f203d18020b9b6eb059a86a1`;
tokio-tungstenite
`24e5bf26aa245eb30c02227c5576265f91cf79d283099d93acfa4a393a722add`;
RSA `bc62327bce4932d5494188d174d67e5754f90c6b932c141c49cd44990026dddc`;
rand `735d731a1e182522dfe469a03a7670791e6fc36fa2d40a80924727c3e65224ef`;
and futures-util
`2c4116cb18d6ab1e63f4a4f55b602684230257108d51c75ec9c19a544b357dbb`.

The metadata scan found no missing license expression in any locked package.
Every expression above is permissive and compatible with this Apache-2.0
project. No GPL, LGPL, AGPL, SSPL, proprietary, or source-available-only
license is admitted by these closures.

## Security and publication decision

1. JWT validation uses **zero clock leeway**. At validation time `now`, the
   exact accepted boundaries are `exp > now` and optional `nbf <= now`:
   `exp == now` is expired and rejected, while `nbf == now` is valid. There is
   no approved skew exception for the local MVP. Issuer, audience, signature,
   `kid`, RS256 algorithm, UUID `sub`, and exact `n2n_role` validation remain
   mandatory.
2. A socket never advances its delivered high-water mark across a sequence
   gap. When a broadcast arrives above `lastSequence + 1`, the gateway replays
   committed rows after `lastSequence` in database sequence order, delivers
   those rows, and only then advances. A later queued duplicate is ignored.
   This makes client delivery robust even if independently scheduled commit
   continuations publish in reverse order.

## Consequences

- Hosts with incorrect clocks fail authentication rather than receiving an
  implicit grace period; deployment health must keep clocks synchronized.
- Gap recovery adds one indexed PostgreSQL replay query only when publication
  order is discontinuous or a receiver lags.
- The audit is tied to the checked-in lockfile. Any dependency or feature
  change requires repeating the closure/license scan and updating this record.
