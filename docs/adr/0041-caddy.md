# 0041. The `caddy` component

Status: Proposed
Date: 2026-11-08
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

Caddy, a third-party web server, is the stack's only entry point. It existed before the rule that each component has its own ADR. This record describes it as it is on `main` after plan step 1.11. It follows [0003](0003-v1-service-layout.md), [0015](0015-certificates.md), [0022](0022-api-path-conventions.md), [0029](0029-shared-origin-protections.md), [0032](0032-container-hardening.md) and [0034](0034-self-signed-https-and-certificate-pinning.md), and links them instead of repeating them.

## Decision

- **What it is:** the Dockerfile's `caddy` target, image `askbar-caddy`: Caddy 2 rebuilt with `xcaddy` and the Cloudflare DNS plugin (both Caddy images pinned by digest, the plugin to a release, [0017](0017-dependencies-and-patches.md)), plus `deploy/caddy/bootstrap.Caddyfile`. The bootstrap file only moves Caddy's admin API onto a unix socket (`/run/caddy/admin.sock`, mode 0660, group 10001); it serves nothing. The admin pushes the real configuration through that socket, and `--resume` brings the last one back after a restart.
- **What it serves**, on one host name ([0022](0022-api-path-conventions.md)):
  - `/admin` and `/api/admin` to the admin's unix socket, open to private addresses by default (`remote_ip` allowlist);
  - `/api/client` (and the old `/api/v1`, which the server answers with 426) to `server:8080`, unbuffered for the chat stream;
  - a `not_found` problem for every other path.
- **Certificates:** until a host name with a real certificate is set, port 443 answers on every address with the install's own certificate, made by the admin ([0034](0034-self-signed-https-and-certificate-pinning.md)). With a host name it gets one from Let's Encrypt over the Cloudflare DNS challenge, reading the token from a file ([0015](0015-certificates.md)). Port 80 only redirects.
- **What it holds:** its certificates and state in `caddy-data` and `caddy-config`. It reads the install's certificate, key and Cloudflare token from `caddy-secrets`, read-only.
- **Volumes:** `caddy-data`, `caddy-config`, `caddy-run` (its admin socket, shared with the admin), `admin-run` (the admin's socket) and `caddy-secrets` (read-only).
- **Networks and ports:** `edge` only. It publishes 80 and 443, the one exception in the stack ([0003](0003-v1-service-layout.md)).
- **Links:** browsers, scripts and clients call it. It calls the admin's socket and the server's client API. The admin configures it through its admin socket.
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with two documented exceptions: it keeps `NET_BIND_SERVICE` to bind 80 and 443, and it runs as `0:10001` so its admin socket belongs to the admin's group and it can read the admin's mode-0640 files without `CAP_DAC_OVERRIDE`. Memory limit 256 MB.

## Alternatives considered

Not recorded: Caddy predates this record.

## Consequences

`check-isolation.sh` tests that Caddy publishes the only ports, refuses its admin API over TCP, serves the install's certificate before a host name is set, routes nothing to the server API, and applies the allowlist as the admin's switch says. The admin's tests check the Caddyfile it generates and that the bootstrap file matches its own copy.

**Where the code and an accepted ADR disagree:**

- [0010](0010-auditor.md): Caddy does not append its access logs to an auditor; there is no auditor yet (plan phase 2).
- [0015](0015-certificates.md) adds HTTP-01 next to the DNS challenge; only the DNS challenge is configured.
