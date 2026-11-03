# Security

## What each part can read

| Part | Can read | Cannot read |
| --- | --- | --- |
| **Server** (`askbar-server`) | conversations, client token hashes, provider API keys (encrypted with a master key only it holds), the audit trace (content sealed) | the admin's database, the admin password hash, the Cloudflare token, the audit passphrase or private key |
| **Admin** (`askbar-admin`) | its own database (password hash, Cloudflare token encrypted, pairing codes, API key hash), the audit trace's metadata through the server API | provider keys (it only sends them), the master key, `audit.db` itself |
| **Caddy** | the Cloudflare token file, TLS certificates | everything else |
| **A client** (the app) | its own conversations; its token only | other users' data, provider keys |
| **An admin API key** | what the admin site can do except managing keys, the password and the session | the same |

Prompts, replies and provider errors are recorded in the audit store only as ciphertext, sealed to a public key
made from the audit passphrase. The server cannot open them; only someone with the passphrase can, in the admin
site, and every opening is recorded. If the passphrase is lost, that content is gone for good. How and why:
[docs/architecture.md](docs/architecture.md).

## What a leaked admin API key can do

It can use the admin API like the site (read audit metadata, change providers and settings, make pairing strings,
download a backup, turn the kill switch on or off). It **cannot** make or revoke keys, change the admin password, open
a session, or open sealed audit content (that needs the passphrase). Revoke it in the admin site, Security tab; it
stops working at once. Only its hash is stored. Wrong keys count toward the same lockout as wrong passwords.

If the admin is reachable from the internet, turn on the address allowlist (Network tab).

## Reporting a problem

Please report security problems privately rather than in a public issue. Use GitHub's private vulnerability
reporting on this repository (the Security tab, "Report a vulnerability"). If that is not available there, open an
issue that says only that you have a security report and asks for a private way to send it, without any details.

We do not run a bug bounty and cannot promise a fix date, but we will answer and credit you if you wish.

## What `scripts/check-isolation.sh` proves

Against a running stack it checks, among other things: the server API answers only with the token and fails closed with
no hash file; only the admin can reach it and the admin joins no other network; the server cannot read the admin's
files or socket; the audit store is mounted by the server only; secret files are mode 600; the server image has no
admin code; backups hold no secrets; the first-run page is reachable on the host's loopback only and not from other
containers; Caddy's loaded route sends only the client API to the server; CLIProxyAPI publishes no port; only the controller mounts the Docker socket and it is on a network with the admin alone; the controller refuses calls without its token and the admin site cannot name the admin or the controller.

### The Docker socket

Starting and stopping services from the admin site needs the Docker socket, which is root on the host. It is mounted in exactly one container, the **controller**, which has no published port, no route out, no capabilities and a read-only filesystem, and answers only the admin (a token whose hash it holds). It can start, stop and restart the `server`, `caddy` and `cpa` services of its own Compose project and nothing else: no create, exec, pull, volume or other containers (`askbar-controller/src/lib.rs`). A compromised admin can therefore stop or restart those three services, but not run anything new. A compromised controller is root on the host; it is small on purpose, and you can leave it out (remove it from `compose.yaml`; the Services tab then says the controller is not set up and everything else works).

It does **not** prove: that the images are free of vulnerable dependencies, that a compromised container cannot attack
the Docker host, that your host name, certificates or DNS are set up well, or that the allowlist you chose is right. It
needs Docker and a throwaway stack; the CI here does not run it.
