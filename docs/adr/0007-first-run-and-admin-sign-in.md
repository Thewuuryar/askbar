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

V1 has one admin account, and no other users have admin access. A domain is needed only for a passkey. Passkeys are optional but recommended in v1.

**First-run flow**

1. Run `docker compose up -d`.
2. Open the host's private address over HTTPS, accept the certificate warning, and sign in with published initial credentials.
3. Change the admin password. The default password or any password matching the username is rejected.
4. Set the audit and backup passphrases.
5. Optionally, enter the URL and DNS settings so Caddy can obtain a real certificate.
6. With a domain, optionally create a passkey and save its recovery codes. The password is then removed. Without a passkey, the admin signs in with the changed password.
7. Add providers and settings.
8. Create a user and generate a pairing key. The user registers a client and starts chatting.

**Requirements**

- Caddy checks the connection's real source, never a forwarded header. Test each supported Docker mode, because source addresses can be rewritten.
- The docs warn that a router or proxy that rewrites the source can make public traffic look private.
- The admin recommends a passkey, but doesn't require one.
- If a passkey is set up, recovery codes replace the password as the backup.
- Each admin reset is recorded in the audit log.

## Alternatives considered

- **A one-time setup code in the logs:** rejected.
- **Loopback-only setup:** rejected because it needs a host browser or SSH tunnel on a headless host.

## Consequences

Until the password changes, any machine on a private network can sign in with the default login, so change it as soon as you deploy. This risk is accepted ([0016](0016-risk-register.md)). If the single admin loses access, recovery codes or the host reset command restore it.
