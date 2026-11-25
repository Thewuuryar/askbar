# 0056. The `egress-proxy` component

Status: Accepted
Date: 2026-11-24
Source: plan phase 4 ([plan-tiered-routing](../plan-tiered-routing/phase-4-egress-proxy.md)); owner decision G8

## Context

[0019](0019-post-v1-order-and-tools.md) puts the egress proxy in the post-v1 order and sets tool rule 2: egress is denied by default and allowed per destination. [0006](0006-v1-outbound-access-and-alerts.md) keeps direct outbound access in v1 and names no proxy component. [0038](0038-names-and-new-components-without-a-name-check.md) sets no name for it. So no ADR decides this component. Today `model-gateway`, `cpa` and `alerter` reach the internet directly over their own networks.

## Decision

- **What it is:** a purpose-built Rust service `services/egress-proxy` (package `askbar-egress-proxy`), compose service and component `egress-proxy`, uid 10006, memory 128 MB, the shared hardening ([0032](0032-container-hardening.md)) with no exception. No third-party proxy image.
- **API:** `/api/egress-proxy/v1` ([0022](0022-api-path-conventions.md)), audience `admin`, link `admin-egress-proxy` (token prefix `admegr_`, volume `egress-proxy-token`, mounted by `bootstrap` only).
- **Source = the network it arrives on.** One internal network per source: `egress-model-gateway`, `egress-cpa`, `egress-alerter`, and `egress-admin` for the admin's API. The proxy has one listener per source, bound to its address on that network and never a wildcard. A source's identity is that network, so no credential is in a URL or variable.
- **The one route out:** `egress-outbound`, not internal, attached to the proxy alone. Caddy keeps its route for certificates. No other service gets a route out.
- **Default deny:** until the admin pushes a config the proxy answers `403` to everything. `PUT /config` replaces the whole config in memory. Nothing is written to disk.
- **Allowlist per source,** pushed by the admin only. Entries are whole ASCII host names with port, `plain_http` and `relay` flags. Private addresses are allowed only by an audited approval (`allow_private`).
- **CONNECT only for HTTPS.** No TLS interception: the proxy sees host and port, never content. Plain HTTP works only for an entry with `plain_http`.
- **Host and address checks:** IP literals in any form are refused unless an entry names them exactly. Every address a name resolves to is checked; one non-public address refuses the connection. The proxy connects to the validated address and never resolves the name again (rebinding defence).
- **SMTP** goes through a per-source relay port that tunnels to one entry's host and port. `lettre` cannot use a proxy, so the TLS name stays the real host. The alerter sends all its traffic this way (ntfy by `CONNECT`, mail by the relay port): it keeps no open network for mail, so [0006](0006-v1-outbound-access-and-alerts.md) gets no exception for mail.
- **`cpa` is limited to Anthropic and OpenAI hosts for now,** the providers behind the two subscription logins. The admin's `CPA_EGRESS_HOSTS` holds only the host names of those two vendors that a trial run shows login and chat need; a host that belongs to neither is refused, and no other provider is allowed for `cpa`. `cpa` reaches them through the proxy by a proxy setting of the pinned CLIProxyAPI (a `proxy-url` key, or Go's `HTTPS_PROXY`): the pin is bumped by digest to a release that has one. An exception that leaves `cpa` on an open network is rejected. If no release supports a proxy, subscription providers are dropped until one does, and the owner is told.
- **Limits and logs:** limits are defaults in `askbar-proto`, pushable within bounds: request head 8 KiB, header lines 64, connect 10 s, head read 10 s, tunnel idle 300 s, 256 tunnels in all and 128 per source. Logs carry source, host, verdict and reason only, never a path or content. `GET /status` counts allowed and denied requests per source and keeps a bounded in-memory list of the 50 most recent denied attempts (source, host, port, reason, time). **Blocked attempts raise no alert:** they are logged and the admin shows them in its UI from its existing poll of `/status`. The proxy has no link to the alerter or the auditor.
- **No persistent state** and **no new crates** beyond the workspace's own (`tokio`, `axum`, `serde`, `tracing`, `askbar-proto`, `askbar-service`).

Invariants, checked by tests and `scripts/check-isolation.sh` once built (see Consequences): the proxy is the only service with a route out besides Caddy; a request from one source is refused on another source's listener; a private address is refused without an approval; no content or secret appears in logs.

## Alternatives considered

- **Squid:** no runtime config API over an authenticated link without a shared volume or file push; its allowlists are per file, not per source network. Rejected.
- **Envoy:** needs an xDS control server to change its config at runtime; that is another service. Rejected.
- **Tinyproxy:** no per-source allowlists and no config API. Rejected.
- **Caddy forward proxy:** mixes the certificate role of Caddy with the egress role, and has no per-source allowlists. Rejected.
- **A third-party proxy image:** rejected by the owner at G8, for the reasons above.
- **A proxy credential in each URL:** rejected by AGENTS.md (no secret in a URL or variable).
- **TLS interception:** rejected. It would put content in the proxy's reach and needs a CA in every client.

## Consequences

- A new container, two kinds of network change (`egress-*` networks replace `outbound` and `alerter-outbound` in the later steps), a credential volume and a link.
- Compose and networks change in P4.2 to P4.3; `scripts/check-isolation.sh` gains the proxy's rows and checks, and the proxy must pass them before the next step.
- **The owner must edit `.github/workflows/ci.yml` and `images.yml`** to build and publish `egress-proxy`. Agents do not change `.github/`.
- Until the admin pushes config, the model gateway, `cpa` and the alerter cannot reach their providers. The push order is part of the design (plan phase 4).
- **Verification** (not run yet, the component is not built): the policy unit tests of P4.1a; the proxy integration tests of P4.1b (listeners, rebinding, limits, API); the isolation proof of P4.5 (default deny, private-address blocking, per-source isolation).
- **Deferred by the owner on 2026-11-24, to decide later:** whether the list of denied attempts must survive a proxy restart (it would need a link to the auditor; not built now).

## Amendments

- 2026-11-24: decided in review of the pull request (#137), no alert for blocked egress (logged, and shown in the admin from `/status`), `cpa` limited to Anthropic and OpenAI hosts for now, and the alerter's mail through the relay with no 0006 exception.
- 2026-11-24: accepted by the owner on 2026-11-24 (PR #137); whether the denied list survives a restart is deferred, and the owner decides it later.
- 2026-11-24: owner's decisions on the open items: tunnel idle 300 s (the other limits as proposed), and `cpa` through the proxy by bumping the pinned CLIProxyAPI, with no 0006 exception and subscription providers dropped if no release supports a proxy.
- 2026-11-25 (proposed, not yet accepted by the owner): the proxy has an `any_public` entry for the `fetch` source (`services/admin/src/egress_link.rs`): it matches every public name, never an IP literal, and private addresses stay refused. It opens port 443 by CONNECT and port 80 with `plain_http`, because the front's HTTP client sends http targets in absolute form. `fetch` has its own listener on `egress-fetch`, and it is the only source with the wildcard. The `searxng` source (`egress-searxng`) gets the exact engine hosts in `SEARXNG_EGRESS_HOSTS` (`services/admin/src/constants.rs`). The list is now the four hosts the owner's trial measured on 2026-11-25 (`scripts/searxng-engine-trial.sh`, query "askbar", one denied host per engine), approved by the owner on 2026-11-25 (G14), and a second run against the pushed list must recheck it. The admin sends both only when the proxy reports their listeners.
