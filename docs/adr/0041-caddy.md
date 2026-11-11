# 0041. The `caddy` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

Caddy, a third-party web server, is the stack's only entry point. It existed before the rule that each component has its own ADR. This record describes it as it is on `main` after plan phase 3. It follows [0003](0003-v1-service-layout.md), [0015](0015-certificates.md), [0022](0022-api-path-conventions.md), [0029](0029-shared-origin-protections.md) and [0032](0032-container-hardening.md), and links them instead of repeating them.

## Decision

- **What it is:** the Dockerfile's `caddy` target, image `askbar-caddy`: Caddy 2 rebuilt with `xcaddy` and the Cloudflare DNS plugin (both Caddy images pinned by digest, the plugin to a release, [0017](0017-dependencies-and-patches.md)), plus `deploy/caddy/bootstrap.Caddyfile`. The bootstrap file only moves Caddy's admin API onto a unix socket (`/run/caddy/admin.sock`, mode 0660, group 10001); it serves nothing. The admin pushes the real configuration through that socket, and `--resume` brings the last one back after a restart.
- **What it serves**, on one host name ([0022](0022-api-path-conventions.md)):
  - `/admin` and `/api/admin` to the admin's unix socket, open to private addresses by default (`remote_ip` allowlist);
  - `/api/client` (and the old `/api/v1`, which the server answers with 426) to `server:8080`, unbuffered for the chat stream;
  - a `not_found` problem for every other path.
- **Certificates:** until a host name with a real certificate is set, port 443 answers on every address with the install's own certificate, made by the admin. With a host name it gets one from Let's Encrypt over the Cloudflare DNS challenge, reading the token from a file ([0015](0015-certificates.md)). Port 80 only redirects. **From plan step 5.3,** the setting `acme_challenge` (`dns` or `http`) chooses the challenge: with `http`, Caddy leaves out the Cloudflare DNS challenge and port 80 also answers the HTTP-01 challenge for the host name ([0015](0015-certificates.md)).
- **What it holds:** its certificates and state in `caddy-data` and `caddy-config`. It reads the install's certificate, key and Cloudflare token from `caddy-secrets`, read-only.
- **Volumes:** `caddy-data`, `caddy-config`, `caddy-run` (its admin socket, shared with the admin), `admin-run` (the admin's socket) and `caddy-secrets` (read-only).
- **Networks and ports:** `edge` (the public side, with a route out), `client-api` (the server's client API, shared with the server alone) and `audit-caddy` (`internal`, shared with the auditor alone). It publishes 80 and 443, the one exception in the stack ([0003](0003-v1-service-layout.md)).
- **Links:** browsers, scripts and clients call it. It calls the admin's socket and the server's client API. Every site sends its access lines, as newline-delimited JSON from Caddy's `net` log writer with no token, to the auditor's ingest listener at `auditor-ingest-host:8086` on `audit-caddy` ([0051](0051-auditor.md)); Caddy still starts when the auditor is down. The admin configures it through its admin socket.
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with two documented exceptions: it keeps `NET_BIND_SERVICE` to bind 80 and 443, and it runs as `0:10001` so its admin socket belongs to the admin's group and it can read the admin's mode-0640 files without `CAP_DAC_OVERRIDE`. Memory limit 256 MB.

## Alternatives considered

Not recorded: Caddy predates this record.

## Consequences

`check-isolation.sh` tests that Caddy publishes the only ports, refuses its admin API over TCP, shares `client-api` with the server alone and `audit-caddy` with the auditor alone (where only the ingest listener answers, and it answers nothing), serves the install's certificate before a host name is set, routes nothing to the server API, and applies the allowlist as the admin's switch says. The admin's tests check the Caddyfile it generates and that the bootstrap file matches its own copy.

**Where the code and an accepted ADR disagree:**

- [0015](0015-certificates.md) adds HTTP-01 next to the DNS challenge; only the DNS challenge is configured (plan step 5.3, `acme_challenge`).

## Amendments

- 2026-11-08: both Caddy images are pinned by digest and the Cloudflare plugin to v0.2.4, so the 0017 mismatch is gone (Thewuuryar/askbar#26).
- 2026-11-11: body brought up to date with the code on `main`: the networks `edge`, `client-api` and `audit-caddy`, and the access lines to the auditor; the 'no auditor yet' disagreement with 0010 is removed. No decision changed.
- 2026-11-11: design for plan step 5.3 (#70, D19): HTTP-01 beside the DNS challenge, chosen by `acme_challenge`; port 80 also serves the challenge. The code PR adds the 'built' line.
