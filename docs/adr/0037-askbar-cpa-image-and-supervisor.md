# 0037. The `askbar-cpa` image and its supervisor

Status: Accepted
Date: 2026-11-07
Source: plan step 1.4 ([plan-v1.md](../plan-v1.md)); owner choice of a supervisor for `cpa`; owner rule that every new component gets an ADR before it is created
Kind: component

## Context

[0005](0005-no-controller-and-self-restart.md) restarts `cpa` (CLIProxyAPI, a third-party image pinned by digest) without a controller: "Test its reload behavior first, and add a small supervisor process only if needed." Plan step 1.4 tested it on the stack with v8.0.51: a new API key written to its config file, by atomic rename or in place, was refused until the container restarted, and comment or `debug` changes were not picked up either. It reads its config only at start. Without a supervisor the admin could not restart it and the person would have to run `docker compose restart cpa` on the host, and connecting `cpa` to `model-gateway` by writing its config would have the same problem. The owner chose the supervisor.

## Decision

- **Image:** a new Dockerfile target `cpa`, image `askbar-cpa`, built `FROM` the same pinned CLIProxyAPI digest. It adds one file, the supervisor binary, and makes it the entry point. CLIProxyAPI itself is unchanged. Updating CLIProxyAPI still means changing the pinned digest ([0017](0017-dependencies-and-patches.md)).
- **Supervisor:** `askbar-cpa-supervisor`, a small Rust program in this repository (`services/cpa-supervisor`), built in the same build stage as the other services (both are Debian bookworm). It runs as the container's main process and:
  - starts `CLIProxyAPI -config /cpa-config/config.yaml` as its child;
  - restarts the child when `config.yaml` changes (checked by content every 2 seconds), when the admin writes a new value to `/cpa-config/restart-request`, or when the child exits on its own (waiting 1 second, doubling to 30, and resetting once it has run for a minute);
  - restarts at most once every 5 seconds;
  - after each start, writes the request value it acted on to `/cpa-config/restart-done`, so the admin can tell the restart happened;
  - on `SIGTERM` or `SIGINT`, passes the signal to the child, waits up to 10 seconds, and exits.
- **Unsafe code:** the only `unsafe` in the services' workspace, in two functions that each allow it: `kill(2)` to send `SIGTERM` to its own child (`std` can send only `SIGKILL`) and `signal(2)` to install a handler that only sets an atomic (`std` has no signal API). Both go through `libc`, with a `SAFETY:` comment. The workspace denies `unsafe_code` everywhere else.
- **From plan step 6.2, export and import:** the admin writes a request file (`export-request` or `import-request`, with the backup public key for an export) to `cpa-config`; the supervisor archives the auth files from `cpa-auth`, encrypts them to that key (`age`) and writes the result to `cpa-config`, or unpacks an import into `cpa-auth` and restarts CLIProxyAPI, and writes a `-done` file naming the request, as it does for a restart. No port, no token ([0013](0013-backups.md), [0042](0042-cpa.md)).
- **No new way in:** the supervisor opens no port and holds no token. The trigger is a file on the `cpa-config` volume, which only the admin and `cpa` mount, as today. A compromised admin could restart `cpa` this way, which it can already do from the Services tab.
- **Everything else is unchanged:** same container name, the networks `cpa-admin`, `cpa-gateway` and `outbound` ([0042](0042-cpa.md)), no published port, the `cpa-config` and `cpa-auth` volumes, the [0032](0032-container-hardening.md) hardening (root, as CLIProxyAPI writes `/root/.cli-proxy-api` on its volume).
- **Admin:** Restart on the Services tab writes a random value to `restart-request`, then waits up to a minute for `restart-done` to show it and for CLIProxyAPI's API to answer. The admin no longer asks for a host restart when it replaces CLIProxyAPI's control secret: the config change restarts it.

## Alternatives considered

- **Host command only (`docker compose restart cpa`):** rejected by the owner. The admin could not restart `cpa`, and step 3.5 would need a host step.
- **A shell-script supervisor:** rejected. Restarts, signal forwarding and backoff in `sh` are easy to get subtly wrong and hard to test; the Rust program is tested with the other services.
- **An HTTP restart endpoint in the supervisor:** rejected. It would need a port on `cpa-admin` and a link token ([0004](0004-bootstrap-and-per-link-tokens.md)) for what a file on a volume the admin already writes can do.
- **Docker's `init` (tini) alone:** rejected. It reaps and forwards signals but cannot restart the child.

## Consequences

- The admin restarts `cpa` from the Services tab, and writing its config (the control secret, and the API key `model-gateway` uses) takes effect without a host step.
- The project builds and ships one more image (`askbar-cpa`); its only code of ours is the supervisor. `check-isolation.sh` checks that the `cpa` container runs the supervisor as its main process, still publishes nothing and is on its own networks only, and that a restart from the admin site restarts CLIProxyAPI without restarting the container.
- The supervisor's tests cover restart on a config change, on a request, after a crash (with backoff), the 5-second limit, and stop-signal forwarding.
- A `cpa` component ADR written in step 1.14 links this ADR for its image and restart.

## Amendments

- 2026-11-08: marked as a component ADR, so it is amended in place ([0046](0046-component-and-register-adrs-and-partial-amendments.md)). Nothing else changed.
- 2026-11-08: records the supervisor's two `unsafe` libc calls, now that the workspace denies `unsafe_code` (contributor guidelines rollout step 3). The code did not change.
- 2026-11-12: `cpa` is on `cpa-gateway`, `cpa-admin` and `outbound`, not `edge` (plan step 3.5, [0042](0042-cpa.md)); the rest of "as today" stands.
- 2026-11-11: body brought up to date with the code on `main`: the supervisor is built and the admin restarts `cpa`; the 'today the admin cannot restart it' wording and the reference to step 3.5 are removed. No decision changed (#70).
- 2026-11-11: design for plan step 6.2 (#70, D26): export and import through request files on `cpa-config`, handled by the supervisor, with no port. The code PR adds the 'built' line.
- 2026-11-12: built export and import (plan step 6.2, PR AK; [0013](0013-backups.md)). Built so: `transfer.rs` in the supervisor. `export-request` is JSON `{id, recipient}`; the supervisor archives the regular files directly in `cpa-auth` (`ASKBAR_CPA_AUTH_DIR`, default `/root/.cli-proxy-api`), encrypts them to the recipient and writes `export.age`, then `export-done` (`{id, ok, bytes}`), without restarting CLIProxyAPI. `import-request` is `{id}` with the plain archive in `import.tar` (the admin opens the backup in PR AM); the supervisor stops CLIProxyAPI, unpacks flat file names into `cpa-auth` (files not in the archive are kept), starts it again and empties `import.tar`, then writes `import-done`. A request is acted on once: it is done when the done file holds its id. The supervisor runs with every capability dropped and cannot create files in the admin's directory, so the admin makes `export-done`, `export.age`, `import-done` and `import.tar` first (mode 0660, as for `restart-done`) and the supervisor writes them in place. The supervisor now links `askbar-service` (its `backup` feature) for `age` and `tar`; it still opens no port and holds no secret (the key is public).
