# 0049. The `bootstrap` component

Status: Proposed
Date: 2026-11-08
Kind: component
Source: plan step 2.1 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07); owner review of the open questions (2026-11-08)

## Context

[0004](0004-bootstrap-and-per-link-tokens.md) decides that a `bootstrap` container creates per-link tokens, and [0005](0005-no-controller-and-self-restart.md) decides that rotation is a host command. The container does not exist yet: today the admin makes the one token it has, the admin-to-server token (`mgt_` and 32 random bytes in base64url, kept in `admin-secrets`), and writes its SHA-256 to `server-token/token.sha256`. The admin mounts `server-token` read-write; the server mounts it read-only. This record describes the component plan step 2.1 builds, and it is proposed before that step so the owner accepts it first. The sources are 0004, 0005, [0032](0032-container-hardening.md) and the Links and Volumes tables in [architecture.md](../architecture.md); this record links them instead of repeating them.

Every container runs `cap_drop: [ALL]` ([0032](0032-container-hardening.md)), so even root cannot read or write another uid's files. A single `bootstrap` must still create files for several readers, and on an existing install the `server-token` directory belongs to the admin's uid (10001).

## Decision

- **What it is:** `services/bootstrap`, a Dockerfile target and image `askbar-bootstrap`. It creates the per-link tokens and exits.
- **No network.** It has no network and publishes no port. It makes and accepts no connection, and no other service calls it.
- **What it holds:** nothing of its own. It mounts credential volumes only, and no data volume. It keeps no state between runs: the tokens and hashes it writes belong to the services that read them ([architecture.md](../architecture.md), Volumes).
- **Who it runs as:** root (`user: "0"`), with `cap_drop: [ALL]` and `cap_add: [CHOWN]` only, the exception [0050](0050-bootstrap-chown-exception.md) adds to 0032. With `CHOWN` alone it takes over a directory it must write, writes the files, then gives each file and directory to its reader. This also covers the upgrade, where `server-token` belongs to the admin.
- **Files and modes:**
  - a token, a secret: owned by the caller's uid, mode `0400`;
  - a hash, not a secret: owned by the receiver's uid, mode `0444`;
  - each directory: owned by its reader's uid, mode `0500`.
- **Links:** a table in code lists each link: its name, caller, receiver and token prefix. A unit test checks the table against the Links table in [architecture.md](../architecture.md). The table holds only links whose receiver exists: in step 2.1, `admin-server`. Each later step adds its rows: the auditor links in step 2.4, `model-gateway` in phase 3, `alerter` in phase 4. The receiver maps each hash to one caller and its allowed routes ([0004](0004-bootstrap-and-per-link-tokens.md)).
- **Token format:** a per-link prefix, then 32 bytes from the OS random generator in base64url without padding (256 bits). `admin-server` keeps the prefix `mgt_`, so the server's parsing does not change. Each new link gets its own short prefix, so a leaked token names its link and secret scanners can match it. The receiver stores the SHA-256 in hex and compares in constant time.
- **Credential volumes:** one per service, named `<service>-token`.
  - `server-token` keeps its name and becomes the server's volume ([architecture.md](../architecture.md): data volumes are never renamed). Step 2.1 adds `admin-token`. Later steps add `auditor-token`, `model-gateway-token`, `alerter-token` and `caddy-token` with their links.
  - Inside each: `call/<link>.token` for the tokens the service uses as a caller, `accept/<link>.sha256` for the hashes it checks as a receiver.
  - Only `bootstrap` mounts them read-write. Each service mounts its own, read-only, and no other.
- **Run once:** it runs before the services that use its tokens and exits. Those services wait for it with `depends_on: condition: service_completed_successfully`. A rerun creates missing tokens and changes none that exist, so a later step only adds rows.
- **Rotation:** from the host, `docker compose run --rm bootstrap` with:
  - `rotate <link>`: one link, by its name, `<caller>-<receiver>` (`admin-server`, `server-auditor`). Where a caller has two links to one receiver, a suffix names each (`admin-auditor-append`, `admin-auditor-read`);
  - `rotate --service <name>`: every link where that service is caller or receiver, for a suspected compromise of one service;
  - `rotate`: every link;
  - `list`: prints the link names.
  - After rotating, it prints the services to restart. The admin then restarts them so they read the new files ([0005](0005-no-controller-and-self-restart.md)). The admin cannot rotate tokens itself.
- **Replaces** the token the admin makes today. The upgrade mints a fresh `admin-server` token, because `bootstrap` does not mount `admin-secrets`. The admin reads its token from `admin-token`, deletes its old `mgt_` file on start, and no longer mounts `server-token`.
- **Not in scope: the `cpa` API key.** CLIProxyAPI compares the plain key, not a hash, so it is not a per-link token. The admin keeps generating it and writing it into `cpa`'s config, as today. Phase 3 decides whether `bootstrap` takes it over, writing it to `model-gateway-token` and to a file `cpa-supervisor` merges into the config, so that the admin no longer holds a credential for a link it is not part of.
- **Volumes:** the credential volumes above, and no other.
- **Networks:** none (`network_mode: none`).
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md) with the one exception in [0050](0050-bootstrap-chown-exception.md). `check-isolation.sh` lists `bootstrap` in its hardening section, checks that its only capability is `CHOWN` and that it has no network, and checks each token and hash file's owner and mode.

## Alternatives considered

- **The admin keeps making tokens:** rejected by [0004](0004-bootstrap-and-per-link-tokens.md). The admin's downtime would be able to expire a service credential.
- **A private CA with mutual TLS, or first-contact claims:** rejected by 0004.
- **Compose file secrets:** rejected by 0004, because those files must exist before startup.
- **One `bootstrap` run per reader uid:** rejected. It needs a run for each uid, and still cannot write into the existing `server-token` directory, which belongs to the admin, not the server.
- **No capability, with setgid directories seeded from the image:** rejected. Docker seeds only an empty, newly created volume, so existing installs would break.
- **One credential volume per link:** rejected. About twelve volumes, all with permanent names, and several mounts per service, for no gain: each service reads only its own volume either way.
- **Tokens for every link in the table from step 2.1:** rejected. It would fix volume names for services whose ADRs are not accepted yet.

## Consequences

Setup needs no manual wiring. A stolen token works until it is rotated ([0016](0016-risk-register.md)). The admin no longer mounts any of the server's volumes. The isolation script has one more service to check, and a missing or wrongly owned token file shows up as a service that fails to start. `bootstrap` runs as root with one capability, for the seconds it runs, with no network.

## Amendments

None yet.
