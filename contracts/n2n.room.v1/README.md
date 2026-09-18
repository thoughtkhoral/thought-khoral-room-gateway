# ThoughtKhoral contracts

`thought-khoral-contracts` is the compatibility authority for the versioned,
language-neutral v1 room-contract JSON Schema artifacts, normative protocol
documentation, and compatibility fixtures.

## Status

MVP / active development. The v1 wire identifier is retained for compatibility;
it is not the public product identity.

## Contents

- [`protocol.md`](protocol.md) — methods, events, errors, and compatibility rules;
- [`schemas/`](schemas/) — JSON Schema Draft 2020-12 artifacts;
- [`fixtures/`](fixtures/) — valid and invalid compatibility examples;
- [`test/validate-fixtures.mjs`](test/validate-fixtures.mjs) — fixture verifier;
- [local specifications](.ai/specs/README.md).

## Validate

Requires Node.js with npm:

```sh
npm ci
npm test
```

The project publishes contract artifacts and does not provide a shared runtime
library. Consumers should pin a released contract tag and verify compatibility
before adopting changes.

## Message mentions and delivery

`chat.send` accepts optional `mentions` and `delivery` fields. The default
delivery is `room`; `mentioned` delivers only to the resolved audience and the
sender. Mentions support direct, known participant targets and the fixed
aliases `@allhumans` and `@allagents`.

Mention tokens are lowercase ASCII slugs, so both `@maya` and `@maya-chen` are
valid forms. A request may include at most 50 unique targets. Participant
targets are unique by ID and aliases by alias, including in persisted
`message.created` payloads. See [`protocol.md`](protocol.md) and the valid and
invalid fixtures for the normative details.

## Contributing

Start with an issue in this repository. Contract changes require an approved
specification update, compatibility evidence, and a release decision. See the
[organization contribution guide](https://github.com/thoughtkhoral/.github/blob/main/CONTRIBUTING.md).
