# 0007. First run and admin sign-in

Status: Accepted 
Date: 2026-11-04
Plan decisions: 6, 7, 34; Finish setup in the admin; Use a single admin

## Context

An admin must be able to deploy Askbar, sign in, and finish setup in a browser. Only private addresses can reach the admin page upon deployment, which is similar to pfSense. 

## Decision

| Area | Askbar |
| --- | --- |
| Who can reach it | Private addresses only. The admin can add Tailscale later |
| Encryption | HTTPS with a self-signed certificate from first boot, and a redirect from port 80 |
| First credential | Published in documentation, then a wizard password step |
| After setup | Allowlist stays on for private addresses, and can be widened in settings |
| Recovery | A host or console command resets the admin account or restores factory defaults |

V1 has one admin account, and no other users have admin access. There is no passkey in v1.

**First-run flow**

1. Run `docker compose up -d`.
2. Open the host's private address over HTTPS, accept the certificate warning, and sign in with published initial credentials.
3. Change the admin password. The default password or any password matching the username is rejected.
4. Set the audit and backup passphrases.
5. Optionally, enter the URL and either the DNS settings (the DNS challenge) or choose the HTTP-01 challenge, so Caddy can obtain a real certificate ([0015](0015-certificates.md)).
6. Add providers and settings.
7. Create a user and generate a pairing key. The user registers a client and starts chatting.

**Requirements**

- Caddy checks the connection's real source, never a forwarded header. Test each supported Docker mode, because source addresses can be rewritten.
- The docs warn that a router or proxy that rewrites the source can make public traffic look private.
- Each admin reset is recorded in the audit log.

## Alternatives considered

- **A one-time setup code in the logs:** rejected.
- **Loopback-only setup:** rejected because it needs a host browser or SSH tunnel on a headless host.

## Consequences

Until the password changes, any machine on a private network can sign in with the default login, so change it as soon as you deploy. This risk is accepted ([0016](0016-risk-register.md)). If the single admin loses access, the host reset command restores it.

## Amendments

- 2026-11-10: two requirements that repeated the passkey rule and first-run step 6 are removed. No decision changed.
- 2026-11-11: no passkey in v1 (#70, owner's decision D20, a decision change): first-run step 6 and the recovery-code text are removed, and the host reset command stays the recovery path; the certificate step names the HTTP-01 choice (D19, [0015](0015-certificates.md)). Passkeys move to the post-v1 list.
