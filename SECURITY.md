# Security

How Askbar v1 limits what a compromise can reach. The risks it defends and the ones it accepts are in the [risk register](docs/adr/0016-risk-register.md).

## What each part can read

| Part | Can read | Cannot read |
| --- | --- | --- |
| **Server** (`askbar-server`) | conversations in every space file, the control database, its own link tokens | stored provider keys, the admin's data, the audit passphrase or private key, audit records |
| **`model-gateway`** | provider keys (in memory), budgets, the prompts and replies it passes through | space files, the admin's data, the audit passphrase or private key |
| **Admin** (`askbar-admin`) | its own data, including provider keys, the Cloudflare token and the password hash; audit metadata | space files, other services' volumes, backup contents; sealed audit content without the passphrase |
| **Auditor** (`askbar-auditor`) | audit metadata, sealed content as ciphertext, the hash chain | sealed content, the audit passphrase or private key |
| **Caddy** | TLS certificates, and the Cloudflare token file (only with the DNS challenge) | everything else |
| **`cpa`** | its config and account logins | everything else |
| **`alerter`** | alert events and its delivery settings | conversations, sealed content |
| **A client** (the app) | its user's personal space | other users' data, provider keys |
| **An admin API key** | what the admin site can do, except managing keys, the password and the session | sealed audit content |

Prompts, replies and provider errors reach the audit store only as ciphertext, sealed to a public key made from the audit passphrase. The server and auditor cannot open them. Only the single admin can, by entering the passphrase in a timed session ([ADR 0011](docs/adr/0011-sealed-content-access.md)); each session and item viewed is recorded, and each unseal sends an alert. If the passphrase is lost, that content is gone for good. The admin also records its own actions in the auditor, with a token that cannot read or change records, and refuses an action if its record cannot be written ([ADR 0010](docs/adr/0010-auditor.md)).

## Exposure

- **Admin:** private addresses only by default; settings can add Tailscale or widen the allowlist. Caddy checks the connection's real source, never a forwarded header. Do not forward the admin page to the internet: a router or proxy that rewrites the source can make public traffic look private.
- **Clients:** the local network or Tailscale by default. Public access is the deployer's choice.
- **First run:** the published login works from private addresses until you change the password. Change it as soon as you deploy.
- **Sign-in:** a password only. There are no passkeys or recovery codes; if you lose it, the host reset command restores access. Setting or rotating the backup key, restoring, deleting a user, lowering retention, creating an API key and changing the password each need a sign-in within the last 10 minutes.

## A leaked admin API key

It can use the admin API like the site: read audit metadata, change providers and settings, make pairing keys, download an encrypted backup, and turn the kill switch on or off. It **cannot** make or revoke keys, change the password, open a session or sealed audit content, or do anything that needs a fresh sign-in. Revoke it on the admin site's Security tab; it stops working at once. Only its hash is stored, and wrong keys count toward the same lockout as wrong passwords.

## Backups

Backups contain secrets, encrypted with age. Each service encrypts its own export (the server, auditor, alerter, `cpa` through its supervisor, and the admin itself; `model-gateway` has none), and the admin bundles them without being able to read the others'. Your backup passphrase protects the private key stored in each backup, so anyone with a stolen file can try to guess it offline: use a long, random passphrase and keep it safe, because it cannot be recovered. A backup needs the passphrase that was current when it was made, and a restore never needs the audit passphrase ([ADR 0013](docs/adr/0013-backups.md)).

## Who is responsible

Askbar has no support or uptime guarantee. The maintainer is responsible for what each release contains. Each deployer is responsible for their host, Docker, patches, alerts, recovery, and any consent, notice or legal duties, which differ for minors, employees and other countries.

## Reporting a problem

Report security problems privately, not in a public issue: use GitHub's private vulnerability reporting on this repository (the Security tab, "Report a vulnerability"). If that is unavailable, open an issue that says only that you have a security report and asks for a private way to send it, with no details.

There is no bug bounty and no promised fix date, but we will answer and credit you if you wish.

## What `scripts/check-isolation.sh` proves

Against a running stack, it checks that:

- Internal APIs answer only with the right token, and fail closed without one.
- Only approved services can reach each link, and services cannot read each other's files or sockets.
- Only the auditor mounts the audit store, and no service token can read, change or delete audit records.
- Secret files are mode 600, and the server image has no admin code. The two Caddy reads (the install's own certificate key and the Cloudflare token) are mode 640 in the admin's group, which only Caddy shares.
- Backups hold secrets only in encrypted form, and no unsealed prompt, reply or provider error appears in the audit store, logs or error text.
- The server cannot open a model-call span.
- No service mounts the Docker socket, `cpa` publishes no port, and every service has the required container hardening.
- Caddy routes only `/admin`, `/api/admin` and `/api/client`, and refuses public sources for the admin.

It does **not** prove that the images are free of vulnerable dependencies, that a compromised container cannot attack the Docker host, that your host name, certificates or DNS are set up well, or that the allowlist you chose is right.

### The Docker socket

No Askbar service has the Docker socket, which is root on the host. The admin requests restarts through each service's own endpoint (a live reload for Caddy), and Docker's restart policy starts the service again. The auditor is restarted from the host ([ADR 0005](docs/adr/0005-no-controller-and-self-restart.md)). A compromised admin can restart services, but cannot use Docker to mount other volumes or run new containers.

An update tool such as Dockhand usually mounts the socket. It is your own tool, outside the Askbar stack, so you accept that risk and should consider a socket proxy.
