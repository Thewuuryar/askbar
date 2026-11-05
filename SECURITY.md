# Security

This document describes the proposed v1 target. The code on `main` doesn't fully match it yet, and each [ADR](docs/adr/README.md) states whether it's Proposed or Accepted.

## What each part can read

| Part | Can read | Cannot read |
| --- | --- | --- |
| **Server** (`askbar-server`) | conversations and memory in every space file, the control database, its own link tokens | stored provider keys, the admin's data, the audit passphrase or private key, audit records |
| **`model-gateway`** | provider keys held in memory, budgets, the prompts and replies it passes through | space files, the admin's data, the audit passphrase or private key |
| **Admin** (`askbar-admin`) | its own data, including provider keys, the Cloudflare token and the password hash; audit metadata | space files, other services' volumes, backup contents; sealed audit content without the passphrase |
| **Auditor** (`askbar-auditor`) | audit metadata, sealed content as ciphertext, the hash chain | sealed content, the audit passphrase or private key |
| **Caddy** | the Cloudflare token file, TLS certificates | everything else |
| **`cpa`** | its config and account logins | everything else |
| **`alerter`** | alert events and its delivery settings | conversations, sealed content |
| **A client** (the app) | its user's personal space and the shared spaces that user belongs to | other users' data, provider keys |
| **An admin API key** | what the admin site can do except managing keys, the password and the session | sealed audit content |

Prompts, replies and provider errors are recorded in the audit store only as ciphertext, sealed to a public key made from the audit passphrase. The server and auditor can't open them. In v1, only the single admin can, by entering the passphrase in a timed session ([ADR 0011](docs/adr/0011-sealed-content-access.md)). Each session and item viewed is recorded, and each unseal sends an alert. If the passphrase is lost, that content is gone for good. The admin also records its own actions in the auditor with a token that can't read or change records, and it refuses an action if its record can't be written ([ADR 0010](docs/adr/0010-auditor.md)).

## Exposure

- **Admin:** it's limited to private addresses by default, and the admin can add Tailscale or widen the allowlist in settings. Caddy checks the connection's real source, never a forwarded header.
- **Clients:** they use the local network or Tailscale by default. Public access is a deployer option.
- **Port forwarding:** a router or proxy that rewrites the source can make public traffic look private. Don't forward the admin page to the internet.
- **First run:** the published `admin:admin` login works from private addresses until you change the password, and nothing else is allowed until you do. Change it as soon as you deploy.
- **Sign-in:** passkeys are optional but recommended, and they need a real domain. Once you create one, recovery codes replace the password as the backup.

## What a leaked admin API key can do

It can use the admin API like the site: read audit metadata, change providers and settings, make pairing keys, download an encrypted backup, and turn the kill switch on or off. It **cannot** make or revoke keys, change the admin password, open a session, or open sealed audit content. Revoke it in the admin site, Security tab; it stops working at once. Only its hash is stored. Wrong keys count toward the same lockout as wrong passwords.

## Backups

Backups contain secrets, encrypted with age. Each service encrypts its own export, and the admin bundles the encrypted exports without being able to read them. Your backup passphrase protects the private key stored in each backup, so anyone with a stolen file can try to guess it offline. Use a long, random passphrase and keep it safe; it can't be recovered. Each backup needs the passphrase that was current when it was made, and a restore never needs the audit passphrase. See [ADR 0013](docs/adr/0013-backups.md).

## Who is responsible

Askbar has no support or uptime guarantee. The project maintainer is responsible for what each release contains. Each deployer is responsible for their host, Docker, patches, alerts, recovery, and any consent, notice, or legal duties, which can differ for minors, employees, and other countries. See the [risk register](docs/adr/0016-risk-register.md) for what v1 defends and what it accepts.

## Reporting a problem

Please report security problems privately rather than in a public issue. Use GitHub's private vulnerability
reporting on this repository (the Security tab, "Report a vulnerability"). If that is not available there, open an
issue that says only that you have a security report and asks for a private way to send it, without any details.

We do not run a bug bounty and cannot promise a fix date, but we will answer and credit you if you wish.

## What `scripts/check-isolation.sh` must prove

The script still checks the current `main` layout. For v1, it must check against a running stack that:

- Internal APIs answer only with the right token, and fail closed without one.
- Only approved services can reach each link.
- Services can't read each other's files or sockets, and only the auditor mounts the audit store.
- Secret files are mode 600, and the server image has no admin code.
- Backups hold secrets only in encrypted form.
- No unsealed prompts, replies or provider errors appear in the audit store, logs or error text.
- The server can't open a model-call span, and no service token can read, change or delete audit records.
- No service mounts the Docker socket.
- Caddy routes only `/admin`, `/api/admin` and `/api/client`, and refuses public sources for the admin.
- `cpa` publishes no port.
- Every service has the required container hardening.

It does **not** prove: that the images are free of vulnerable dependencies, that a compromised container cannot attack the Docker host, that your host name, certificates or DNS are set up well, or that the allowlist you chose is right.

### The Docker socket

No Askbar service has the Docker socket, which is root on the host. The admin requests restarts through each service's own endpoint, or a live reload for Caddy, and Docker's restart policy starts the service again. The auditor is restarted from the host ([ADR 0005](docs/adr/0005-no-controller-and-self-restart.md)). A compromised admin can restart services, but it can't use Docker to mount other volumes or run new containers.

If you use an update tool such as Dockhand, its usual setup mounts the socket. It's your own tool, outside the Askbar stack, so you accept that risk and should consider a socket proxy.
