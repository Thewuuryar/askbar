# 0019. Post-v1 order and tools

Status: Accepted 
Date: 2026-11-04
Plan decisions: 25, 26

## Context

Web search and research are the first features wanted after v1. They let untrusted web content influence a model that may also have private context.

## Decision

**Order:** ship the egress proxy, then the MCP gateway, `searxng`, web research, and the judge together, then the executor.

**Tool rules**

1. Tools run in two containers: the executor for untrusted code, and the MCP gateway for credentialed tools. Neither has a route to `/api/server/v1` or the admin API. The executor has no secret volumes, and the MCP gateway holds only tool credentials.
2. Network egress from tool containers is denied by default and allowed per destination.
3. Every tool call, command, network request, and file change is an audit span written before it runs. If the audit write fails, the call doesn't run.
4. Tool output is untrusted text. It can't change settings, create a client, or reach the admin API.
5. Per-user token limits and the kill switch cover tool use.
6. The client popup window still gets no filesystem or network permission.

The server runs the agent loop, so each tool action passes through a single audited point.

**Models:** the admin can choose a provider and model separately for research agents and for the judge.

- Both roles go through `model-gateway`, with usage tracked per role against budgets.
- Every model or provider change is recorded in the audit log.
- If the judge's model is unavailable or over budget, actions that need its review are refused.

**Web research controls**

1. Fetch only URLs from search results or the user.
2. Make search and fetch separate per-user permissions, off by default.
3. Require approval when a fetch is requested in a conversation that has used private memory, or when a URL carries query data.
4. Block private addresses, limit size and time, and mark all output as untrusted.
5. Fetched text can't write memory or trigger tools without confirmation.
6. Record each search and fetch before it runs, including the full URL.

## Alternatives considered

- **One tool container:** rejected, because untrusted code and credentials must not share a container.
- **Judge after web research:** replaced by shipping them together.
- **Executor before web research:** rejected, because web research needs no code execution.

## Consequences

The judge can be prompt-injected, so these deterministic controls must stand on their own. An egress allowlist doesn't stop data leaving through a URL when the tool must reach the open web. Judge usage counts toward model budgets. This work doesn't block v1.
