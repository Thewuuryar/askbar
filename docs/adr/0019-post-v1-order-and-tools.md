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

## Amendments

- 2026-11-24: accepted by the owner (decision change; PR #136): the order of work after 1.0 is (0) the refactor in [plan-refactor.md](../plan-refactor.md), each stage landing before the track 1 stage that needs it, with no behaviour change; (1) step records; (2) the egress proxy; (3) the MCP gateway and `searxng`; (4) quick search; (5) research reports with the judge; (6) PII detection and redaction; (7) context checks and canaries; (8) tiered routing; (9) agents with MCP tools; (10) audit viewing; (11) the executor and the script library, both deferrable. Roles use admin-pinned models until tiered routing. Quick search runs without the judge, under the fixed rules in [plan-priorities.md](../plan-priorities.md), because nothing it fetches can cause another tool call. The judge ships with research reports, the first feature with a tool loop. This replaces the Decision's order line; the order is set here only, per [plan-priorities.md](../plan-priorities.md) step O0.1.
- 2026-11-25 (accepted by the owner): the MCP gateway is an own front, `mcp-gateway` (`services/mcp-gateway`), with native tools `search` and `fetch`; no third-party MCP gateway is used. The fetch rules are enforced in the front before any network call (`services/mcp-gateway/src/fetch_rules.rs`): a `search` URL must be among the task's remembered results, a query string only when `query_data` is signed, http and https only, ports 80 and 443 only, no userinfo, no non-ASCII host, no IP literal in any form, no local-looking name. Every redirect hop re-runs the rules (a cross-host redirect is refused for `search` provenance and allowed for `user`; https to http is refused), and each hop opens a `tool` span that the auditor acknowledges before the request. The client sends `Accept-Encoding: identity` and refuses any other Content-Encoding. The egress proxy resolves names and is authoritative for private ranges. robots.txt is not read (owner decision open).
- 2026-11-25 (accepted by the owner): fetch decodes gzip (the owner chose a decompression crate: flate2, pure Rust, miniz_oxide backend), advertising Accept-Encoding: gzip only; the size cap applies to the decoded bytes; any other Content-Encoding is still refused
- 2026-11-25 (accepted by the owner): fetch converts HTML to text with html2text 0.16 (MIT; html5ever-based, default features off; the owner chose an HTML crate), not a hand-written extractor
- 2026-11-25 (accepted by the owner): robots.txt is not read for user-initiated searches (this settles the open point in the MCP gateway amendment above).
