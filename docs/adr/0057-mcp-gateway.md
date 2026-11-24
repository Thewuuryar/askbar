# 0057. The `mcp-gateway` component

Status: Proposed
Date: 2026-11-24
Kind: component
Source: plan phase 5A ([phase-5a-mcp-gateway](../plan-tiered-routing/phase-5a-mcp-gateway.md)); owner decisions G13 and G16

## Context

[0019](0019-post-v1-order-and-tools.md) puts tools after v1 and sets the rules for them. [0001](0001-decision-records-and-change-rules.md) rule 3 needs an ADR for a new service and image. Askbar's only tools through 1.0 are `search` and `fetch` ([0002](0002-v1-scope-limits-and-ship-criteria.md)); federation to other MCP servers is not needed for them. Whatever calls a tool, one front must check the approval signature, open the audit span before the call, check the permission and apply the fetch rules.

## Decision

- **Own front, native tools, no third-party gateway** (owner decision G13, 2026-11-24). A gateway would not remove the front, because the front must do those four checks anyway. It would add a pinned image, a scan gap and upstream breaking releases, for federation that `search` and `fetch` do not use.
- **Name and place:** component and compose service `mcp-gateway`, crate `services/mcp-gateway` (package `askbar-mcp-gateway`), uid 10007, built on `askbar-service`. The name was checked against [0021](0021-names-and-new-components.md) rule 4 and [0038](0038-names-and-new-components-without-a-name-check.md) (plan A12) and is kept: no existing component or term is confusable with it. Hardening is the shared block of [0032](0032-container-hardening.md), no exception. It holds only tool credentials, none today.
- **API:** `/api/mcp-gateway/v1` ([0022](0022-api-path-conventions.md)), REST with problem+json, described in `docs/openapi/mcp-gateway.v1.yaml`. No MCP wire code in any other component.
- **Links** ([0049](0049-bootstrap.md)): `server-mcp-gateway` (the server calls the front), `admin-mcp-gateway` (the admin pushes config) and `mcp-gateway-auditor` (the front writes `tool` spans). It has no route to `/api/server/v1`, the admin API or `model-gateway`.
- **Networks,** all `internal` but the proxy's: `mcp-server`, `mcp-admin`, `audit-mcp-gateway`, `mcp-searxng` (front and `searxng` only), `egress-searxng` and `egress-fetch`. Nothing else joins them.
- **Egress:** default deny, through the proxy ([0056](0056-egress-proxy.md)) with two sources of its own, `searxng` (exact engine hosts) and `fetch` (one `any_public` entry, ports 80 and 443). `fetch` has its own source so a wildcard-public identity is never shared with a credentialed server.
- **Check order on a call:** config present (else fail closed) → approval → tool permission → open the audit span and wait for the acknowledgement → run the tool → close the span. A failure at any step refuses with a distinct problem+json code before anything runs.
- **Approval:** Ed25519 with `ring`, in a new optional `approval` feature of `askbar-service`; `askbar-proto` holds the types, no crypto. The action hash is SHA-256 over canonical JSON. Verify checks signature, hash, 60 s expiry and a one-time ID.
- **Fetch rules** (G15): enforced in the front, listed in [How it works](../plan-tiered-routing/phase-5a-mcp-gateway.md#how-it-works-the-plans-decisions-the-mcp-gateway-adr-records-them-in-p5a2), not copied here.
- **Output is untrusted data;** `search` and `fetch` are each a per-user permission, off by default.
- **`searxng`:** the unmodified upstream image (AGPL-3.0, a separate container; source link in the third-party notices), pinned by digest, in a derived image that adds only the settings file and a start script writing a random `secret_key` into tmpfs. Hardened per 0032. Settings and the non-root spike are in the plan file.
- **Registry:** a tool name maps to a native handler. A second kind, upstream MCP server (`rmcp`, added at G16), is future work and changes no client API; it is not built now.
- **Each further MCP server** passes the [G16 checklist](../plan-tiered-routing/phase-5a-mcp-gateway.md#g16-checklist-for-each-further-mcp-server).
- **No new crates** beyond `ring` (a direct, optional dependency of `askbar-service`) and the workspace's own.

Invariants once built: no call runs before its audit span is acknowledged; no unsigned, replayed, swapped or expired approval runs a tool; the front has no route to the server or admin APIs.

## Alternatives considered

- **agentgateway:** runner-up. Kept as an option for one upstream behind the same front at G16.
- **ContextForge:** too heavy for two native tools.
- **Docker MCP Gateway:** needs the Docker socket, which no service gets (AGENTS.md).
- **Microsoft MCP Gateway:** needs Kubernetes.
- **Envoy AI Gateway, Kong, LiteLLM, MetaMCP and others:** too heavy, not open source, or licence-split.

## Consequences

- A new container and image, a new image for `searxng`, networks, links and an API prefix. The owner must add both images to `.github/workflows/ci.yml` and `images.yml`; agents do not change `.github/`.
- Not built yet, so not enforced. Tests are the plan's P5A steps; `scripts/check-isolation.sh` gains the networks' rows.
- Until the admin's first config push the front refuses every call.
- Amendments to other ADRs, each its own step: [0019](0019-post-v1-order-and-tools.md) (P5A.24), [0028](0028-service-boundaries-and-config-delivery.md) (P5A.25), [0032](0032-container-hardening.md) (P5A.26), [0017](0017-dependencies-and-patches.md) (P5A.27), [0026](0026-dependency-license-check.md) (P5A.28), [0016](0016-risk-register.md) (P5A.29), [0056](0056-egress-proxy.md) (P5A.30), [0036](0036-askbar-service-crate.md) (P5A.31), [0022](0022-api-path-conventions.md) (P5A.32), [0049](0049-bootstrap.md) (P5A.33), [0010](0010-auditor.md) (P5A.34), [0051](0051-auditor.md) (P5A.35).

## Amendments

(none yet)
