# 0040. The `admin` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

The admin existed before the rule that each component has its own ADR. This record describes it as it is on `main` after plan step 1.11. It follows [0003](0003-v1-service-layout.md), [0005](0005-no-controller-and-self-restart.md), [0007](0007-first-run-and-admin-sign-in.md), [0022](0022-api-path-conventions.md), [0028](0028-service-boundaries-and-config-delivery.md), [0029](0029-shared-origin-protections.md), [0032](0032-container-hardening.md) and [0034](0034-self-signed-https-and-certificate-pinning.md), and links them instead of repeating them.

## Decision

- **What it is:** `services/admin`, binary and image `askbar-admin`, the Dockerfile's `admin` target. It runs as uid and gid 10001. It serves the admin site at `/admin` and its API at `/api/admin/v1`, on a unix socket only (`/run/admin/admin.sock`); it opens no TCP port.
- **What it holds:**
  - `admin.db`: the admin's password hash, admin API key hashes, users, providers (no keys except the managed `cpa` one, below), settings, display defaults, budgets and the kill switch, network settings (the Cloudflare token encrypted with `admin.key`), pairing-code hashes, the audit keyring (public keys and salts only, [0030](0030-sealed-audit-content.md)) and its record of who opened sealed content;
  - in `admin-secrets`: `admin.key`, the server API token, and `cpa`'s API key and control secret;
  - in `caddy-secrets`: the install's own certificate and key ([0034](0034-self-signed-https-and-certificate-pinning.md)) and the Cloudflare token file Caddy reads.
- **Volumes:** `admin-data`, `admin-secrets`, `server-token` (it writes the token's hash for the server), `admin-run` (its socket, for Caddy), `caddy-run` (Caddy's admin socket), `caddy-secrets`, and `cpa-config` (it writes `cpa`'s config and restart requests, [0037](0037-askbar-cpa-image-and-supervisor.md)).
- **Networks:** `server-api` (fixed address `172.30.0.3`) and `cpa-admin`. Both are `internal`, so the admin has no route out. Caddy reaches it only through its socket.
- **Links:** it calls the server API with its token, pushes Caddy's configuration through Caddy's admin socket, and calls `cpa`'s management API with the control secret. Nothing calls it except Caddy, for a person's browser or a script with an admin API key.
- **Configuration push:** every change is saved in `admin.db` and pushed to the server as one snapshot; if the server cannot be reached the change is refused and nothing is saved ([0028](0028-service-boundaries-and-config-delivery.md)). It polls the server's status and pushes again after a restart.
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with no exception; memory limit 256 MB.

## Alternatives considered

Not recorded: the admin predates this record.

## Consequences

`check-isolation.sh` tests that the admin works with the server stopped, that the server cannot reach its files or socket, and that its backup holds no secret. Its own tests and `tests/e2e/admin.test.js` cover the site and API.

**Where the code and an accepted ADR disagree.** Each is a later plan step, not a decision to keep it:

- [0010](0010-auditor.md): the admin does not record its own actions in an auditor, and so cannot refuse an action whose record fails; there is no auditor yet (plan phase 2). It records who opened sealed content in `admin.db` only.
- [0014](0014-model-gateway-and-providers.md): the admin does not store ordinary providers' keys; it passes them to the server, which stores them. In v1 the admin stores them and `model-gateway` holds them in memory (phase 3).
- [0004](0004-bootstrap-and-per-link-tokens.md): the admin makes the server API token and writes its hash to `server-token`, a volume it shares with the server; `bootstrap` will do this (plan step 2.1).
- [0013](0013-backups.md): the backup is an unencrypted tar that the admin can read, so secrets are left out (phase 6).
- [0003](0003-v1-service-layout.md) (no shared volumes): the admin shares `server-token` with the server, `caddy-run`, `caddy-secrets` and `admin-run` with Caddy, and `cpa-config` with `cpa`. These carry tokens, sockets and configuration, not another service's data; the target layout keeps the Caddy and `cpa` ones and moves the token to `bootstrap`.

## Amendments

None yet.
