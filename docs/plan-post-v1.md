# Plan: after v1

Status: tracker for work after 1.0. The order and rules are decided in [ADR 0019](adr/0019-post-v1-order-and-tools.md) (egress proxy, then web research with the judge, then the executor) and [ADR 0020](adr/0020-remote-executor-and-ca-triggers.md) (executor pairing, tunnel, CA triggers). This file keeps the v2 picture and the repo decisions that are not yet an ADR. Container hardening proposals are in [plan-container-hardening.md](plan-container-hardening.md).

## V2 sketch

V2 adds an egress proxy, web research with a judge, and then an executor, all under the v1 trust model. The server still runs the agent loop, so each tool action passes through a single audited point. The admin still pushes configuration, services still append to the auditor, the controller stays dropped, and the admin stays on one host name unless a later decision changes that.

```mermaid
---
title: V2 adds one route out and a private executor link
---
flowchart TB
    clients["Clients"]
    adminUser["Admin user"]

    subgraph host["Askbar host"]
        caddy["<b>caddy</b><br/>Only published ports. The one service<br/>outside the egress proxy, for certificates"]
        server["<b>server</b><br/>Chats and space files<br/>Runs the agent loop"]
        auditor["<b>auditor</b><br/>Append-only log, hash chain<br/>No outbound route"]
        admin["<b>admin</b><br/>Secrets, settings, pairing<br/>CA only if a trigger is met"]
        judge["<b>judge (new)</b><br/>Reviews gray areas<br/>Own provider, model"]
        modelGateway["<b>model-gateway</b><br/>Provider keys, budgets,<br/>kill switch"]
        mcp["<b>MCP gateway (new)</b><br/>Credentialed tools,<br/>web research"]
        link["<b>executor-link (new)</b><br/>Optional Tailscale<br/>service, one port"]
        cpa["<b>cpa</b><br/>Optional login"]
        searxng["<b>searxng (new)</b><br/>Web search"]
        alerter["<b>alerter</b><br/>Auditor alerts, releases"]
        egress["<b>egress proxy (new)</b><br/>Default deny, with an allowlist per source<br/>Blocks private addresses except approved hosts such as the GPU desktop"]
    end

    internet["Internet and GPU desktop"]
    executor["<b>executor (new)</b><br/>Home server or VPS<br/>Own egress proxy, no secrets"]

    clients -->|"local network or Tailscale"| caddy
    adminUser -->|"HTTPS"| caddy
    caddy -->|"/api/client/v1"| server
    caddy -->|"/admin socket"| admin
    admin -->|"config"| caddy
    server --> auditor
    admin --> auditor
    server -->|"server calls"| judge
    server -->|"server calls"| modelGateway
    server -->|"server calls"| mcp
    link -->|"work and output"| server
    judge --> modelGateway
    modelGateway --> cpa
    mcp --> searxng
    modelGateway --> egress
    mcp --> egress
    cpa --> egress
    searxng --> egress
    alerter --> egress
    egress --> internet
    executor -->|"Tailscale"| link

    classDef new stroke-width:3px
    class judge,mcp,link,searxng,egress,executor new
```

Thick outline and "(new)": new in v2.

## Open repo decisions

Not decided anywhere yet. Each becomes an ADR, a rule in [AGENTS.md](../AGENTS.md) or [CONTRIBUTING.md](../CONTRIBUTING.md), or an issue.

- **Swift app conventions.** Decide when the iOS app starts: `swift-format` or `SwiftLint`, and the same error-code mapping as the desktop app.
- **An accessibility baseline for the popup.** Not decided anywhere. Suggest: every action reachable by keyboard (already true), visible focus, and contrast checked in both themes (`scripts/check-ui-tokens.sh` already checks the token contrast).
- **AI attribution.** Keep a `Co-authored-by:` trailer on agent commits so history shows it. Recommended; not written into CONTRIBUTING.md.
- **Keeping the rules alive.** Add a rule only after something went wrong or was debated, and add the check that catches it in the same change. A rule with no command that verifies it belongs in a review checklist, not in AGENTS.md. Recommended; not written into CONTRIBUTING.md.
