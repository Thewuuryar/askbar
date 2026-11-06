# 0034. Self-signed HTTPS and certificate pinning

Status: Accepted
Date: 2026-11-07
Source: required by [0007](0007-first-run-and-admin-sign-in.md); owner choice in the v1 plan ([plan-v1.md](../plan-v1.md), steps 1.6 and 1.7)

## Context

[0007](0007-first-run-and-admin-sign-in.md) serves HTTPS with a self-signed certificate from first boot, before any host name or DNS exists, and [0015](0015-certificates.md) leaves that certificate in use until a real one is issued. Clients reach the host by its address, so no public CA can vouch for it, and the desktop client refuses an untrusted certificate. Caddy's internal CA could make the certificate, but only for names it knows; the host's addresses are unknown inside the container, so it would need on-demand issuance.

## Decision

- **One certificate per install.** On its first start the admin makes one self-signed certificate (ECDSA P-256) and its key, and writes them to the volume it shares with Caddy for secrets: certificate 0644, key 0600. It never replaces them on its own.
- **Any name.** Until a host name with a real certificate is set, Caddy serves port 443 of every address with this certificate, for any server name or none (`default_sni`, `fallback_sni`). Port 80 redirects to HTTPS.
- **The pairing key names it.** While this certificate is in use, a pairing key carries its SHA-256 as `cert_sha256` ([wire contract](../wire-contract.md#pairing)). A pairing key made with a real certificate carries none.
- **The client trusts exactly that certificate.** A client paired with a pin accepts the server only if its certificate has that SHA-256, and checks the handshake signature; the name and issuer are not checked. The pin is kept in `client.yaml` (it isn't a secret). A client without a pin uses the public roots. A different certificate is refused, and the client says to pair again.

Invariants a test checks: the certificate is made once and kept; the key file is 0600; a pairing key carries the pin exactly when the install's own certificate is in use; a pinned client refuses any other certificate.

## Alternatives considered

- **Caddy's internal CA:** rejected. It issues for known names only, so it needs on-demand TLS with an `ask` endpoint, and the client would have to trust a CA rather than one certificate.
- **Trust on first use:** rejected. The first connection would be unauthenticated; the pairing key is already a trusted channel from the admin.
- **`allow_insecure` (accept any certificate):** kept for development only. It authenticates nothing.
- **Requiring a real certificate before pairing:** rejected. Private installs without a domain couldn't pair.

## Consequences

A fresh install pairs over HTTPS by address with no domain. Browsers still warn on the admin site until a real certificate exists. Replacing the certificate (deleting it from the volume) means pairing every pinned client again. Clients paired with a pin keep it after a real certificate is set, until they are paired again. Enforced by tests in `services/admin` (`self_cert`, pairing) and the desktop client (`pinned_tls`, pairing), and by section 6 of `scripts/check-isolation.sh`.
