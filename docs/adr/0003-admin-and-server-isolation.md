# 0003. The admin and the server are isolated

Status: Proposed (the code on `main` still uses the `server-token` volume and the server's `master.key`; see "When implemented")
Date: 2026-11-04 (first written 2026-11-03)

## Context

Goal from the goals document: "Keep admin separate, so running agents can never change settings", and "nothing on the chat side can write back to admin". The server talks to model providers, serves the client API and handles untrusted text; the admin holds the settings and the password. The owner's priority is a compromised server, ahead of a compromised admin, and keeping every path a model could use to break containment as short as possible.

## Decision

1. **Separate containers, uids, volumes and images.** The server cannot read the admin's data. The admin cannot read the server's database. The audit store is kept by the auditor, not the server ([0009](0009-the-auditor.md)); the admin reads it through the auditor's API.
2. **The admin holds every secret and the server holds none at rest.** Provider keys, the Cloudflare token and the other configuration secrets live in the admin's storage. The admin sends the server only what it needs, over the server API, in the configuration snapshot. The server keeps provider keys in memory. It has no `master.key` and no `askbar-secrets` volume.
3. **The server never calls the admin.** Until the admin has pushed a snapshot, the server answers `503 not_configured`. The admin checks a status route on the server API (`GET /status`, reporting the configuration version) and pushes when the server is unconfigured, for example after a restart.
4. **The only link is the server API.** It listens on one fixed address on an `internal` network that only the admin and the server join. No volume is shared between them.
5. **The server trusts the admin by claim on first contact.**
   - The admin generates a server token and keeps it. On its first call, the server stores a hash of that token in its own database. From then on the server accepts only that token.
   - The claim is accepted only while no token is stored and only from the admin's fixed address on the `server-api` network.
   - Resetting the stored token is an operator command run on the server container. Nothing over the network can do it.
   - The server token is used for nothing else. The controller has its own token.
6. **A rejected token is treated as a compromise.** If the admin was accepted once and then gets a rejection, it shows an alert and offers to stop the server through the controller. Stopping does not depend on the server cooperating.

## Alternatives considered

- **Token hash on a read-only shared volume** (what `main` does). The server cannot change it, but the two containers share a mount. Rejected: the owner wants no shared volume between them.
- **A one-shot init job that writes each side's half.** No claim window and a clean rotation. Not chosen: it adds a container, and the owner accepts an operator reset instead. Revisit if resets become routine.
- **Mutual TLS, or a unix socket with a peer-uid check.** More to operate, or a new shared mount, for little gain between two containers.
- **Trust by network membership alone.** Rejected: any container added to `server-api` later would gain admin powers.
- **Provider keys encrypted in the server under a `master.key`** (what `main` does). The key and the ciphertext sit in the same container, so it protects little beyond a copied database.

## Consequences

- A stolen server volume or backup holds no provider keys. A compromised running server still holds the keys in memory, because it must call providers. Removing that needs a separate model gateway, which is out of scope here.
- A compromised server can delete its stored token and re-claim from its own address. The address check refuses that, but the check relies on the server being unable to forge a source address, which needs the container hardening in [plan-container-hardening.md](../plan-container-hardening.md). Without it, the worst outcome is that the admin is locked out and sees a rejection (item 6).
- The admin becomes the one place holding every secret and is reachable from the internet ([0002](0002-one-name-per-role-one-host.md)). Its sign-in ([0008](0008-admin-sign-in-and-exposure.md)) carries more weight.
- A server restart needs the admin to be running to push the snapshot. Steady state does not.
- Restoring a server backup restores an old stored token, so the operator must reset it before the admin can reconnect.
- The `control-token`, `caddy-secrets` and `cpa-config` volumes are not covered here; they belong to the controller and third-party components ([0005](0005-docker-socket-only-in-controller.md)).

## When implemented

Remove the `server-token` volume and the server's `askbar-secrets` volume. Update `SECURITY.md` (what each part can read), `AGENTS.md` (the hard rules on shared volumes and provider keys), `architecture.md` (volumes and networks), the OpenAPI file for the server API (`GET /status`, `not_configured`, the claim) and `scripts/check-isolation.sh`. The isolation script should also fail if `server-api` has any member besides the admin and the server, and prove that the server's volumes hold no provider key material. Existing installs: the owner re-enters provider keys in the admin.
