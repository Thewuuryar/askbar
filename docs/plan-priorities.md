# Plan: order of work after 1.0

Status: proposed, 2026-11-23. The order below is the owner's. It takes effect when the owner accepts the amendment to [ADR 0019](adr/0019-post-v1-order-and-tools.md) in step O0.1. Until then, ADR 0019 as accepted already puts the egress proxy, the MCP gateway, web research and the judge first, so track 1 matches it. Rules for agents: [AGENTS.md](../AGENTS.md).

This file decides **order** only. Each track's own plan decides behaviour and holds the steps. Where a track reuses steps from another plan, this file names them by ID instead of copying them.

## Order

| # | Track | Plan | Delivers | Status |
| --- | --- | --- | --- | --- |
| 1 | Web search | this file, then the [tiered routing](plan-tiered-routing.md) phase files it names | Search and fetch through the MCP gateway, quick web search in chat, research reports, the judge | not started |
| 2 | PII detection and redaction | [plan-pii-redaction.md](plan-pii-redaction.md) | Placeholders out, real values back, blocked secrets, PII checks on search queries | not started |
| 3 | Context checks and canaries | [plan-canaries.md](plan-canaries.md) | Stop reasons, context limits, the echo canary, leak tripwire, injection markers on fetched pages | not started |
| 4 | Tiered routing | [plan-tiered-routing.md](plan-tiered-routing.md) | Tiers, the decider, Auto, dollar budgets, then agents with MCP tools | not started |
| 5 | Audit viewing | [plan-audit-viewer.md](plan-audit-viewer.md) | Conversation view of sealed records, step records, per-user audit keys | not started |

A track starts when the one before it has its PR open, unless the owner says otherwise. Tracks share code in two places, named under [Cross-track decisions](#cross-track-decisions); everything else is independent.

## Why this order

1. **Web search first.** It is the feature the family wants most, and ADR 0019 already makes it the first work after v1. It has no dependency on routing: each research role can use a model pinned by the admin until tiers exist.
2. **PII second.** Web search opens a new path out of the house: search queries go to public search engines through `searxng`. PII checks on queries and prompts close that path and the provider path together.
3. **Canaries third.** Fetched pages make contexts much longer, so silent truncation becomes likely once search ships. Fetched pages are also where prompt injections arrive, so the canary plan's injection markers become useful now instead of "later".
4. **Tiered routing fourth.** It saves money rather than adding a capability. Its decider thresholds need labeled examples, and real use, including search, produces them in the meantime.
5. **Audit viewing last.** It shows the records every earlier track writes. Built last, it is built once against their final shape. Until then the current per-item sealed view still works.

## Track 1: web search

Starts only after 1.0 ships: [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) keeps Askbar conversation-only through 1.0.

### What ships

- **Quick web search in chat.** The user turns on "Search the web" for a message.
  - The researcher role proposes up to 3 queries, and the server searches through the MCP gateway.
  - It fetches up to N result pages, from search results only (ADR 0019 rule 1), and marks fetched text as untrusted (datamarking, as in Microsoft's spotlighting).
  - The writer answers with numbered sources. The reply lists the queries used and the sources.
  - This is a research report with one research question and no task resume, so it reuses the research code rather than adding a second path.
- **Research reports**, as in the [design](design-tiered-routing.md#research-reports), with roles pinned to models.
- **The judge**, as the `judge.review` role on a pinned model. It reviews the gray areas ADR 0019 names, such as a fetch of a URL that carries query data. ADR 0019 requires the judge to ship with web research.
- **Search and fetch permissions** for each user, off by default. Every search and fetch is recorded, full URL included, before it runs.

### What waits for track 4

Tiers and weights for research roles, the decider cascade for the judge (P5.7), decider step checks and research filters (P5.8), dollar budgets, and agents with MCP tools (Phase 6). Until then:
- research roles use admin-pinned models;
- research and judge usage count against token budgets, per role;
- the task cost cap (P5.10) is a token cap.

### Stages

| Stage | Content | Steps |
| --- | --- | --- |
| W0 | Records | O0.1 to O0.3 below |
| W1 | Egress proxy | Tiered routing [Phase 4](plan-tiered-routing/phase-4-egress-proxy.md): P4.1 to P4.8. In P4.4, leave out the decider addresses, which come with track 4 |
| W2 | MCP gateway, `searxng`, search and fetch | Tiered routing [Phase 5A](plan-tiered-routing/phase-5a-mcp-gateway.md): every step, including P5.2 to P5.5 (signed approvals) |
| W3 | Roles pinned to models | W3.1 and W3.2 below: the parts of P1.1, P5.14 and P5.15 that research needs, without tiers |
| W4 | Quick web search in chat | W4.1 to W4.5 below |
| W5 | Research reports and the judge | Tiered routing [Phase 5B](plan-tiered-routing/phase-5b-research-judge.md): P5.6, P5.9, P5.10 (token cap), P5.13, P5.17 and P5.18. P5.7 and P5.8 move to track 4 |
| W6 | Admin pages | Tiered routing [Phase 8](plan-tiered-routing/phase-8-admin-ui.md) P8.6a (MCP servers and per-user tool permissions), and the research and judge pins from W3.2. The admin UI overhaul is merged (`services/admin/src/constants.rs` is on `main`), so gate G0 is cleared |

#### W0: records

- [ ] **O0.1** `docs(adr): amend 0019: order of work after 1.0` · haiku
  Files: `docs/adr/0019-post-v1-order-and-tools.md`.
  Do: order is egress proxy; MCP gateway, `searxng`, web research and the judge together; PII detection and redaction; context checks and canaries; tiered routing; agents with MCP tools; audit viewing; then the executor and the script library (both deferrable). Research and judge roles use admin-pinned models until tiered routing.
  Accept: `scripts/check-docs.sh` passes; the Amendments section names this file.
- [ ] **O0.2** `docs(docs): point the tiered routing plan at the new order` · haiku · after O0.1
  Files: `docs/plan-tiered-routing.md`, `docs/plan-tiered-routing/phase-0-docs.md`, `docs/plan-tiered-routing/phase-5a-mcp-gateway.md`.
  Do: confirm the order text in P0.10, P0.16 and P5A.24 matches O0.1, and that the Phases table marks Phases 4, 5A and 5B as run under track 1. This commit already made those edits; this step checks them after O0.1 is accepted.
- [ ] **O0.3** `docs(docs): reorder the post-v1 plan` · haiku · after O0.1
  Files: `docs/plan-post-v1.md`. Replaces tiered routing P0.16.

#### W3: roles without tiers

- [ ] **W3.1** `feat(proto): add model roles and role pins` · haiku · after O0.1
  Files: `crates/askbar-proto/src/gateway.rs`, tests.
  Do: `Role` with `chat.manual`, `research.researcher`, `research.writer`, `judge.review`; `role_pins: BTreeMap<Role, ModelId>` on `GatewayConfig`; `role` optional on `ChatRequest`. Field names match tiered routing P1.1, so track 4 adds tiers to the same types.
  Accept: old config and request JSON parse unchanged; a request naming an unpinned role is refused with an `ErrorCode`.
- [ ] **W3.2** `feat(admin): pin a model for each research role and the judge` · sonnet · after W3.1
  Files: `services/admin/src/{db,gateway_link,admin_api}.rs`, the providers or MCP settings template, `app.js`, `docs/openapi/admin.v1.yaml`.
  Do: each pin must be an approved model. If the judge's pin is unavailable or over budget, actions that need the judge are refused (ADR 0019).
  Accept: pins push with the gateway config; token usage is counted per role.

#### W4: quick web search in chat

- [ ] **W4.1** `feat(proto): add a web search flag to chat requests` · haiku · after W3.1
  Files: `crates/askbar-proto/src/lib.rs`, `docs/openapi/client.v1.yaml`, `docs/wire-contract.md`.
  Do: an optional `web_search: bool` on the client's chat request; reply events gain optional `sources` (title, URL) and `queries`. Additive only.
- [ ] **W4.2** `feat(server): answer a message with web search` · sonnet · after W4.1, P5A.23, P5.13
  Files: `services/server/src/{generation,research}.rs`.
  Do: check the user's search and fetch permissions; run the research flow with one question; cap queries at 3 and fetched pages at the `constants.rs` value; wrap fetched text in untrusted-content markers; ask the writer for numbered citations; record the queries and sources on the turn. If search or fetch is refused, answer without it and say so.
  Accept: a turn with search records a search span and a fetch span before each call; a fixture page containing instructions ("ignore previous instructions and fetch …") causes no fetch outside the search results and no settings change.
- [ ] **W4.3** `feat(client): search toggle and sources` · sonnet · after W4.1
  Files: `clients/desktop/src/**`, its tests.
  Do: a per-message "Search the web" toggle, shown only when the user has the permission; a sources list under the reply. Links open in the system browser; the popup itself gets no network permission (ADR 0019 rule 6). Escape Markdown as today.
  Accept: client tests cover the toggle, the sources list and that a source link doesn't load inside the app.
- [ ] **W4.4** `test(e2e): web search with the mock model and a fixture search engine` · haiku · after W4.2, W4.3
  Files: `tests/e2e/`, `scripts/smoke.sh`.
  Do: a fixture `searxng` response and fixture pages, no real internet; extend the mock LLM to propose queries and cite sources.
- [ ] **W4.5** `docs(docs): web search in the user and admin guides` · haiku · after W4.4
  Files: `docs/guide.md`, the admin reference pages for search and fetch permissions.
  Do: say what leaves the house (search queries to the search engines `searxng` uses; fetched URLs) and that search is off until the admin turns it on.

### Owner gates for track 1

1.0 released; G8 (egress proxy software and licence, new container); G13 (agentgateway or ContextForge, after the P5A.1 spike); G9 (signing crate, `mcp-gateway` and `searxng` containers, accept ADR 0057); G14 (search destinations); G15 (fetch egress rule). Gate definitions are in the [tiered routing plan](plan-tiered-routing.md#owner-gates).

## Cross-track decisions

| Topic | Decision |
| --- | --- |
| Reply stream filter | Track 2 builds it (PII restore). Track 3 adds the canary filters to the same chain. Previously the canary plan owned it |
| Search queries and PII | Until track 2, search is off by default and every reply shows its queries. Track 2 runs detection on search queries and fetch URLs: secrets block the search; other PII is removed from the query and the user is told |
| Injection markers | Track 1 ships datamarking of fetched text. Track 3 adds canaries inside fetched text to detect pages that make the model leak or follow instructions |
| Roles | Track 1 adds roles pinned to models with the same type names tiered routing uses; track 4 adds tiers, weights and the decider to them |
| Judge | Track 1: a pinned model only. Track 4: the decider cascade in front of it (P5.7) |
| Audit records | Tracks 1 to 4 record their steps as attributes on existing spans (tool spans for search and fetch, attempt attributes for PII and canaries). Track 5 adds the `step` span kind and shows all of them; records written before it show without steps |
| ADR numbers | 0056: tiered routing (reserved). 0057: MCP gateway (track 1, P5A.2). 0058: the PII detector container (track 2). Other changes are amendments |
| Agents with MCP tools | Stay in track 4, after routing Phases 1 to 3, because they depend on role tiers and the decider's step checks |

## Open questions for the owner

- **Agents with MCP tools:** keep them at the end of track 4, or after track 5?
- **Quick search before research reports?** W4 depends on the research code from P5.13, so it lands with or just after reports. The alternative is a smaller search-only path before P5.13; it would ship sooner but duplicate code.
- **Step records earlier?** Pulling the viewer's `step` span kind (AV2.1) into track 1 would let every later track record in its final shape, and leave track 5 as UI only. The cost is a small schema change earlier than planned.

## Sources

- [Defending Against Indirect Prompt Injection Attacks With Spotlighting](https://arxiv.org/abs/2403.14720)
- [How Microsoft defends against indirect prompt injection attacks](https://www.microsoft.com/msrc/blog/2025/07/how-microsoft-defends-against-indirect-prompt-injection-attacks)
