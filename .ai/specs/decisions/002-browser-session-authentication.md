# 002 — Browser initial-message authentication profile

- **Status:** Accepted
- **Approved at:** 2026-09-11T21:10:23Z
- **Approval basis:** Approved integration correction adopting root Decision 002
- **Parent authority:** [Root Decision 002 — Browser WebSocket authentication](../../../../.ai/specs/decisions/002-browser-websocket-authentication.md)
- **Scope:** `n2n-room-gateway` MVP WebSocket boundary
- **Parent override:** None. This decision implements the parent browser-authentication rule and preserves the prior non-browser agent path only as described below.

## Decision

The gateway consumes contract release `n2n-room-v1.0.2`, which adds
`session.authenticate` as the sole pre-authentication JSON-RPC request.

An upgrade carrying an HTTP `Origin` header is a browser-path request. Its
origin must exactly match one of the normalized `http` or `https` origins in
the required `N2N_ALLOWED_ORIGINS` comma-separated allowlist. An origin-bearing
upgrade never derives identity from an HTTP `Authorization` header. After the
upgrade, its first application message must be `session.authenticate` and must
arrive within `N2N_SESSION_AUTH_TIMEOUT_MS`, which defaults to 5000 and is
restricted to 1 through 30000 milliseconds.

The authentication message carries only `accessToken`. The existing OIDC JWT
validator verifies issuer, audience, signature, signature-use `kid`, RS256,
UUID `sub`, exact role, `exp`, and optional `nbf`, then binds the resulting
actor to that socket. Success returns the request id with
`result.actor.{id,role}` and integer `result.expiresAt`. Token failure returns
`-32001`; a room method before authentication also returns `-32001`. Either
case closes with WebSocket policy code 1008 after the error when possible.
Malformed initial requests return their contract error and close. Missing the
authentication deadline returns `-32001` when possible and closes with 1008.

A no-`Origin` client is treated as a non-browser agent and retains the existing
`Authorization: Bearer` upgrade requirement. It cannot use omission of Origin
to establish an unauthenticated socket. URL query parameters and
`Sec-WebSocket-Protocol` never carry credentials in either path.

Every authenticated socket is closed at the validated token expiration
instant using the same `-32001`/1008 policy. Reauthentication and token refresh
on an already-bound socket are not supported by this MVP.

Rejected-path logs contain only error codes and already validated identifiers.
They never include access tokens, authorization headers, raw request bodies,
raw room content, or authentication response material.

## Consequences

- Browser clients can use the standard WebSocket API without putting tokens in
  URLs or protocol headers.
- Origin validation happens before an unauthenticated browser socket exists;
  application authentication then limits that socket to one safe message.
- Non-browser bearer compatibility does not create a browser fallback because
  every origin-bearing request remains on the browser path.
