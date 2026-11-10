# 0049. The `bootstrap` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 2.1 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner review of the open questions (2026-11-08)

## Context

[0004](0004-bootstrap-and-per-link-tokens.md) decides that a `bootstrap` container creates the per-link tokens, and [0005](0005-no-controller-and-self-restart.md) that rotation is a host command. Today the admin makes the one token there is: the admin-to-server token, kept in `admin-secrets`, with its SHA-256 in `server-token/token.sha256`. Every container drops all capabilities ([0032](0032-container-hardening.md)), so even root cannot write another uid's files. Yet one `bootstrap` must create files for several readers, and on an existing install `server-token` belongs to the admin's uid (10001).

## Decision

- **What it is:** `services/bootstrap`, image `askbar-bootstrap`. It creates the per-link tokens and exits.
- **What it holds:** nothing. It mounts credential volumes only, and keeps no state between runs.
- **Who it runs as:** root with `CHOWN` as its only capability ([0032](0032-container-hardening.md) rule 6). It takes over a directory it must write, writes the files, then gives each file and directory to its reader; that also covers the upgrade, where `server-token` belongs to the admin.
- **Files and modes:**
  - a token, a secret: owned by the caller's uid, mode `0400`;
  - a hash, not a secret: owned by the receiver's uid, mode `0444`;
  - each directory: owned by its reader's uid, mode `0500`.
- **Links:** a table in code gives each link's name, caller, receiver and token prefix, and a unit test checks it against the Links table in [architecture.md](../architecture.md). It holds only links whose receiver exists: `admin-server` in step 2.1, the auditor links in 2.4, `model-gateway` in phase 3, `alerter` in phase 4.
- **Token format:** a per-link prefix, then 32 random bytes from the OS in unpadded base64url. `admin-server` keeps `mgt_`; each new link gets its own prefix, so a leaked token names its link and secret scanners can match it. The receiver stores the SHA-256 in hex and compares in constant time.
- **Credential volumes:** one per service, named `<service>-token`.
  - `server-token` keeps its name and becomes the server's. Step 2.1 adds `admin-token`; later steps add `auditor-token`, `model-gateway-token`, `alerter-token` and `caddy-token`.
  - Inside each: `call/<link>.token` (tokens it sends) and `accept/<link>.sha256` (hashes it checks).
  - Only `bootstrap` mounts them read-write. Each service mounts its own, read-only, and no other.
- **Run once:** services wait for it (`depends_on: condition: service_completed_successfully`). A rerun creates missing tokens and changes none that exist.
- **Rotation:** from the host, `docker compose run --rm bootstrap` with:
  - `rotate <link>`: one link, named `<caller>-<receiver>`, with a suffix where a caller has two links to one receiver (`admin-auditor-append`, `admin-auditor-read`);
  - `rotate --service <name>`: every link of one service, for a suspected compromise;
  - `rotate`: every link; `list`: the link names.
  - It then prints the services to restart, and the admin restarts them ([0005](0005-no-controller-and-self-restart.md)).
- **Replaces** the admin's token. The upgrade mints a fresh `admin-server` token (`bootstrap` does not mount `admin-secrets`); the admin reads it from `admin-token`, deletes its old file on start, and no longer mounts `server-token`.
- **Not in scope: the `cpa` API key.** CLIProxyAPI compares the plain key, not a hash, so the admin keeps writing it into `cpa`'s config. Phase 3 decides whether `bootstrap` takes it over, so that the admin holds no credential for a link it is not part of.
- **Volumes:** the credential volumes only. **Networks:** none (`network_mode: none`); it makes and accepts no connection.
- **Hardening:** [0032](0032-container-hardening.md)'s defaults with its rule 6. `check-isolation.sh` checks that `CHOWN` is its only capability, that it has no network, and each token and hash file's owner and mode.

## Alternatives considered

- **One run per reader uid, or setgid directories seeded from the image:** rejected ([0032](0032-container-hardening.md)). Neither can write into the existing `server-token`, and Docker seeds only a new, empty volume.
- **One credential volume per link:** rejected. About twelve volumes, all with permanent names, and several mounts per service, for no gain: each service reads only its own volume either way.
- **Tokens for every link in the table from step 2.1:** rejected. It would fix volume names for services whose ADRs are not accepted yet.

The admin making the tokens, a private CA with mutual TLS, first-contact claims and Compose secrets were rejected in [0004](0004-bootstrap-and-per-link-tokens.md).

## Consequences

The admin no longer mounts any of the server's volumes. A missing or wrongly owned token file shows up as a service that fails to start. The other consequences (no manual wiring, a stolen token works until rotated) are [0004](0004-bootstrap-and-per-link-tokens.md)'s.

## Amendments

- 2026-11-10: trimmed: the alternatives and consequences that 0004 and 0032 already record are linked, not restated, and links to the merged 0050 point to 0032. No decision changed.
- 2026-11-10: built in plan step 2.1 (PR: feat/bootstrap). Built so: with every capability dropped, root cannot read or stat another uid's directory, so `bootstrap` takes each credential volume over (owner itself, mode 0700), writes, then gives every file and directory to its reader. A link is complete only when its token and its hash both exist; otherwise both are made again. `ASKBAR_BOOTSTRAP_ROOT` (development runs only) puts the volumes under a directory owned by the current user.
