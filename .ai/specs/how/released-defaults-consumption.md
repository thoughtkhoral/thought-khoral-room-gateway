# Released defaults contract consumption

Status: approved local implementation on 2026-11-20 by the maintainer's
“proceed” after verified v1.1 publication and the stated broker/UI transition.
This implements Task A of the approved root/platform Task 9 procedure under
existing component issue 1 and the conversation requirements. No new product
behavior, dependency, service activation, source publication or provider call
is authorized by this local change.

## Verified input and layout

Consume release `thought-khoral-agent-conversation-v1.1.0`, source
`f1c8621810c8c121c1aafc8e5a342d6626c4b843`. The downloaded TAR SHA-256 is
`b0370e43627ce6044852df75bfbc4b749d904aa9cedcfc558c61bb4e07dc95ff`;
release-lock SHA-256 is
`b7f3ad928c5c17be7e936da4959b7e7e8555bb4ed3602fa62f71a8721f042d8f`.
Publication and fresh-download byte verification are recorded in the root
coordinated plan and the contracts publication manifest. Import the exact lock
and its 156 repository-relative payload files to
`contracts/agent-conversation-v1.1/`. Do not copy archive-only source files,
sidecars or a fabricated relabeled candidate lock into the vendor directory.

Keep existing `contracts/agent-conversation-v1` and
`contracts/agent-conversation-v1.1-candidate` byte-for-byte unchanged. The new
release payload matches all candidate payload hashes; only release provenance
changes. Production resolved-settings schema loading switches to the released
directory; retained schemas remain on their published v1.0 pins.

## Verification

Before artifact/import changes, add focused failing release-pin coverage.
Pin the external lock digest in the owning verifier; require exact release
metadata, source, archive URL/digest, complete file/directory set and every
payload hash. Reject altered/missing/extra payload, metadata changes, root or
nested symlinks and unsupported entries. Confirm identical bytes with the old
candidate payload and preserved published v1.0 schemas/fixtures. Retain all
historical candidate checks and synthetic source pins.

Broker: extend Rust protocol/integrity test coverage and switch only the
resolved-settings include path in src/conversation_protocol.rs. Run protocol,
defaults and relevant conversation tests with unchanged Cargo locks; database
checks require an isolated temporary database and may not be silently skipped.

UI: add scripts/check-default-settings-release.mjs and its executable tamper
checks, run them in pretest/prebuild beside retained checks, and switch only
the resolved-settings import in conversationApi.ts. Run the full UI suite and
TypeScript/production build with unchanged dependency locks. No visual component
or user flow changes are introduced.

Record actual final clean commits for platform consumption. Separate review
covers these exact changes before a packaged live profile freezes them. Root
identity-gate compatibility allows only the exact retained contractVersion
field in ordinary-human-message.json and ordinary-codex-message.json beneath
the new release vendor; it grants no broader path or legacy-name exception.
Publication, merges, live activation and provider checks retain separate gates.
