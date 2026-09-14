# Contracts implementation

Follow the [root N:N MVP foundation implementation plan](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/how/n2n-mvp-foundation-implementation-plan.md) and the root governance decision before changing this project.

Implementation begins only after the relevant task is approved. The implementation produces language-neutral JSON Schema, protocol documentation, and compatibility fixtures without creating a shared runtime library.

## Browser WebSocket authentication patch

The accepted root [browser WebSocket authentication decision](https://github.com/thoughtkhoral/thought-khoral/blob/main/.ai/specs/decisions/002-browser-websocket-authentication.md) governs browser clients. This contract records `session.authenticate` as an additive `n2n.room.v1` patch: it is the only JSON-RPC method permitted before a connection is authenticated and its `params` object contains only a non-empty string `accessToken`. Existing authenticated room-method schemas and fixtures remain unchanged.

The gateway owns normative runtime behavior: it validates the token and its issuer, audience, signature, key identifier, algorithm, expiry, and not-before claims; it permits no room operation before successful authentication; and it closes a connection that fails authentication or misses the configured short authentication timeout. Schemas validate the message shape only and must never encode, log, or retain access tokens.

## Contract validation

The fixture verifier uses the maintained `ajv` 8.20.0 release as its validation dependency. Ajv is MIT licensed and its official documentation provides the dedicated `ajv/dist/2020` export required for JSON Schema Draft 2020-12; the project uses that export rather than the default Draft 07 validator. The verifier also uses the companion `ajv-formats` 3.0.1 package so UUID and RFC 3339 `date-time` formats are assertions rather than annotations. Both packages are development-only dependencies; Node's standard library loads fixtures and no shared runtime package is produced.

### Dependency and license verification

Checked 2026-09-11. Exact installed versions, npm registry tarball URLs, integrity hashes, and SPDX license declarations are locked in the committed `package-lock.json`; the matching installed source packages' `package.json` and license files were inspected before adoption. All licenses below are compatible with this repository's JSON-schema and documentation artifact distribution.

| Package | Version and authoritative source evidence | License | Compatibility conclusion |
| --- | --- | --- | --- |
| `ajv` | `8.20.0`; [upstream source](https://github.com/ajv-validator/ajv), [Draft 2020-12 documentation](https://ajv.js.org/json-schema.html); `package-lock.json` resolves `https://registry.npmjs.org/ajv/-/ajv-8.20.0.tgz` and the installed package declares the same version. | MIT | Compatible. Its dedicated `ajv/dist/2020` export supports the required Draft 2020-12 dialect. |
| `ajv-formats` | `3.0.1`; [upstream source](https://github.com/ajv-validator/ajv-formats); `package-lock.json` resolves `https://registry.npmjs.org/ajv-formats/-/ajv-formats-3.0.1.tgz` and its source package declares `ajv: ^8.0.0` as both dependency and peer dependency. | MIT | Compatible. The locked Ajv 8.20.0 satisfies `^8.0.0`; this package supplies the UUID and date-time format validators used by the fixture verifier. |
| `fast-deep-equal` | `3.1.3`; [upstream source](https://github.com/epoberezkin/fast-deep-equal); `package-lock.json` resolves `https://registry.npmjs.org/fast-deep-equal/-/fast-deep-equal-3.1.3.tgz`. | MIT | Compatible. It is an Ajv transitive implementation dependency and has no contract-runtime API exposure. |
| `fast-uri` | `3.1.7`; [upstream source](https://github.com/fastify/fast-uri); `package-lock.json` resolves `https://registry.npmjs.org/fast-uri/-/fast-uri-3.1.7.tgz`. | BSD-3-Clause | Compatible. Its permissive BSD-3-Clause terms are compatible with the artifact distribution; it is an Ajv transitive URI-processing dependency with no contract-runtime API exposure. |
| `json-schema-traverse` | `1.0.0`; [upstream source](https://github.com/epoberezkin/json-schema-traverse); `package-lock.json` resolves `https://registry.npmjs.org/json-schema-traverse/-/json-schema-traverse-1.0.0.tgz`. | MIT | Compatible. It is an Ajv transitive schema-walking dependency with no contract-runtime API exposure. |
| `require-from-string` | `2.0.2`; [upstream source](https://github.com/floatdrop/require-from-string); `package-lock.json` resolves `https://registry.npmjs.org/require-from-string/-/require-from-string-2.0.2.tgz`. | MIT | Compatible. It is an Ajv transitive compilation dependency with no contract-runtime API exposure. |

The selected graph contains only Ajv, the Ajv format extension, and their five locked transitives. No dependency ships in a runtime library or is used to handle untrusted requests outside the development fixture verifier.

The verifier treats the RPC schema as the fixture entry point, registers the envelope and room-event schemas by stable `$id`, and requires every valid fixture to validate and every invalid fixture to fail. Schema and protocol changes remain spec-first and must be released under the compatibility rule in `protocol.md`.
