# 0008. Admin sign-in and exposure

Status: Proposed (the location is settled; sign-in is open)
Date: 2026-11-03

## Context

On `main` ([0002](0002-one-name-per-role-one-host.md)): the admin is at `/admin` on the shared host name, signs in with a password (or an admin API key for scripts), rate-limits and locks out after failures, and has an address allowlist that is **off by default**.

Settled on 2026-11-03: the admin stays at `/admin` on one host name and one certificate, in its own isolated container. Still open from the goals document: passkey sign-in with password and TOTP as backup, and "view user data" as an explicit action.

## Decision (proposed)

- Add passkeys (WebAuthn) as the primary sign-in. Keep the password as the backup. Add TOTP only if you want a second backup.
- Keep "view user data" as the existing sealed-audit opening: explicit, per item, recorded in the access history ([0004](0004-sealed-audit-content.md)).
- Ask during first-run setup whether to turn the allowlist on, instead of leaving it off silently.

## Alternatives considered

- **Admin on a subdomain.** Rejected, see [0002](0002-one-name-per-role-one-host.md).
- **Admin only over Tailscale.** Strongest exposure story; needs the Tailscale item from the goals document first.

## Questions to answer before accepting

1. Is the admin ever reached from the internet, or only from the home network or a VPN? That decides how much the allowlist and passkeys matter.
2. Passkeys now, or after the iPhone app?
3. Is TOTP worth building if the password stays as backup?

## Tools to evaluate

`webauthn-rs` and `totp-rs` are the Rust crates to look at first. Check their maintenance and licence (this repo is AGPL-3.0-or-later) before adopting either.

## Consequences

If accepted: the admin API and `docs/openapi/admin.v1.yaml` gain session and credential routes (additive within `v1`), `SECURITY.md` gets a new "what a stolen password can do" section, and the admin tests cover each sign-in path.
