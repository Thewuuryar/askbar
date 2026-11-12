# 0015. Certificates

Status: Accepted
Date: 2026-11-04
Plan decisions: 20, 34

## Context

A deployment may use a private address or a public host. A domain isn't required. [0007](0007-first-run-and-admin-sign-in.md) serves HTTPS from first boot, before any host name or DNS exists. Clients then reach the host by its address, so no public CA can vouch for it, and the desktop client refuses an untrusted certificate.

## Decision

**Real certificates.** V1 keeps Caddy's Cloudflare DNS plugin and adds HTTP-01, chosen by the setting `acme_challenge` (`dns` or `http`) on the Network page, which warns that HTTP-01 needs public port 80:

| Option | Works with | Needs | Private address |
| --- | --- | --- | --- |
| Cloudflare DNS | Cloudflare | API token limited to one zone | Yes |
| HTTP-01 | Any DNS provider | Public port 80 | No |

Other DNS plugins, or the acme-dns module, are added only when someone needs them.

**Until then, a self-signed certificate, pinned at pairing:**
- **One per install.** On its first start the admin makes one self-signed certificate (ECDSA P-256) and its key. It writes them to the volume it shares with Caddy for secrets: certificate 0644, key 0600. It never replaces them on its own.
- **Any name.** Until a host name with a real certificate is set, Caddy serves port 443 of every address with it, for any server name or none (`default_sni`, `fallback_sni`). Port 80 redirects to HTTPS.
- **The pairing key names it.** While it is in use, a pairing key carries its SHA-256 as `cert_sha256` ([wire contract](../wire-contract.md#pairing)). A pairing key made with a real certificate carries none.
- **The client trusts exactly that certificate.**
  - A client paired with a pin accepts the server only if its certificate has that SHA-256, and checks the handshake signature. The name and issuer are not checked.
  - The pin is kept in `client.yaml`; it isn't a secret.
  - A client without a pin uses the public roots.
  - A different certificate is refused, and the client says to pair again.

Invariants a test checks:
- the certificate is made once and kept;
- the key file is 0600;
- a pairing key carries the pin exactly when the install's own certificate is in use;
- a pinned client refuses any other certificate.

## Alternatives considered

- **More DNS plugins, or acme-dns, now:** rejected until someone needs them. Each adds a build dependency and testing, and acme-dns needs a service to run or trust.
- **Caddy's internal CA for the default certificate:** rejected. It issues for known names only, which would need on-demand TLS with an `ask` endpoint, and the client would trust a CA rather than one certificate.
- **Trust on first use:** rejected. The first connection would be unauthenticated, while the pairing key is already a trusted channel from the admin.
- **`allow_insecure` (accept any certificate):** kept for development only. It authenticates nothing.
- **A real certificate before pairing:** rejected. Private installs without a domain couldn't pair.

## Consequences

- A fresh install pairs over HTTPS by address, with no domain. Private deployments that want a real certificate use Cloudflare DNS.
- Browsers warn on the admin site until a real certificate exists.
- Replacing the self-signed certificate (deleting it from the volume) means pairing every pinned client again. A pinned client keeps its pin after a real certificate is set, until it is paired again.
- Enforced by tests in `services/admin` (`self_cert`, pairing) and the desktop client (`pinned_tls`, pairing), and by section 6 of `scripts/check-isolation.sh`.

## Amendments

- 2026-11-10: merged in 0034 (self-signed HTTPS and certificate pinning), as "Until then, a self-signed certificate, pinned at pairing", with its alternatives and consequences.
- 2026-11-11: no passkey in v1 (#70, D20, a decision change): a domain is no longer mentioned as needed for one; and HTTP-01 is chosen by the setting `acme_challenge` (D19), built in plan step 5.3.
- 2026-11-12: built HTTP-01 (plan step 5.3, PR AG, #84). Built so: `network_settings.acme_challenge` (`dns` or `http`, appended migration 19, default `dns`); with `http` HTTPS needs no Cloudflare token (the stored one stays, and is not given to Caddy); `PUT /network` refuses `http` for a host name with a private suffix (`.local`, `.lan`, `.internal`, `.home`, `.home.arpa`, `.localdomain`, `.localhost`, `.corp`, `.intranet`, `.private`, `.test`, `.example`, `.invalid`), which Let's Encrypt cannot reach, and `dns` without a token now says `http` is the way out; the Network page offers the choice and warns that port 80 must reach this machine from the internet and that the name must point at the public address. No real issuance was tested: it needs a public host name.
