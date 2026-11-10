# 0003. V1 service layout

Status: Accepted
Date: 2026-11-04
Plan decisions: 2, 5, 13

## Context

The service layout must limit what a compromise can reach while staying small enough for one owner to run.

## Decision

- **Services:** v1 runs `caddy`, `server`, `admin`, `cpa`, `auditor`, and `model-gateway`. It also has a small `alerter` and a one-time `bootstrap`.
- **Host name:** the admin stays on one host name, with one admin URL and no separate hostname switch or admin ingress. The switch can be revisited in v2.
- **Published ports:** only `caddy` publishes ports. It attaches to a non-internal network, and this exception is documented.
- **Server:** it stores no provider keys or admin data, only its own link tokens ([0004](0004-bootstrap-and-per-link-tokens.md)), and has no outbound route.
- **Audit and configuration:** `server`, `model-gateway`, and `alerter` append to the auditor with their own tokens. Caddy appends only its access logs, and `cpa` and `bootstrap` don't append. The admin's token is limited to admin action events, and the admin refuses an action if its record can't be appended ([0010](0010-auditor.md)). The admin also reads the auditor and pushes configuration and restart requests to each service.
- **Storage:** each service has its own data and credential volumes and user ID, with no shared data volumes or host bind mounts.

**Design records**

- A component matrix lists what each service holds, what it can reach, and what its compromise exposes, and is tested with `check-isolation.sh`.
- A failure table records behavior, alerting, and recovery for each service, including a full auditor disk.
- A network map shows every link, including config pushes, audit appends, alerts, and certificate requests.
- The emergency stop uses a `server` or `model-gateway` policy flag for per-user pause, and keeps alert delivery running during level 1.
- Each delivery step has acceptance tests and a rollback.

## Alternatives considered

- **A separate admin hostname:** rejected for v1. It would add another host name, certificate, Cloudflare token, and container.

## Consequences

A compromised service reaches only its own volumes and approved links. Host root can still read everything ([0016](0016-risk-register.md)).

## Amendments
