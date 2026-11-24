# Plan: order of work after 1.0

Status: proposed, 2026-11-23; owner decisions added 2026-11-24. The order below is the owner's. It takes effect when the owner accepts the amendment to [ADR 0019](adr/0019-post-v1-order-and-tools.md) in step O0.1. Rules for agents: [AGENTS.md](../AGENTS.md).

This file decides **order** and the few things the order changes. Each track's own plan decides behaviour and holds the steps. Where a track reuses steps from another plan, this file names them by ID instead of copying them.

## Order

| # | Track | Plan | Delivers | Status |
| --- | --- | --- | --- | --- |
| 1 | Web search | this file, then the [tiered routing](plan-tiered-routing.md) and [audit viewer](plan-audit-viewer.md) steps it names | Step records, search and fetch through the MCP gateway, quick search in chat, then research reports and the judge | not started |
| 2 | PII detection and redaction | [plan-pii-redaction.md](plan-pii-redaction.md) | Placeholders out, real values back, blocked secrets, PII checks on search queries | not started |
| 3 | Context checks and canaries | [plan-canaries.md](plan-canaries.md) | Stop reasons, context limits, the echo canary, leak tripwire, injection markers on fetched pages | not started |
| 4 | Tiered routing | [plan-tiered-routing.md](plan-tiered-routing.md) | Tiers, the decider, Auto, dollar budgets, then agents with MCP tools at the end | not started |
| 5 | Audit viewing | [plan-audit-viewer.md](plan-audit-viewer.md) | Conversation view of sealed records, per-user audit keys | not started |

A track starts when the one before it has its PR open, unless the owner says otherwise.

## Owner decisions (2026-11-24)

1. **Step records come first.** The audit viewer's `step` span kind and payload roles (AV0.2, AV2.1 to AV2.3) move to the start of track 1, so every track records in its final shape. Track 5 keeps the view, per-user keys, search and browser decryption.
2. **Quick search ships before research reports.** It is a small path of its own, not a one-question research report. Research reports and the judge follow it in track 1.
3. **Agents with MCP tools stay at the end of track 4.** No web search step depends on them: Phases 4, 5A and 5B name no Phase 6 step, and quick search uses no planner or worker role.

## Why this order

1. **Web search first.** It is the feature the family wants most, and ADR 0019 already makes it the first work after v1. It needs no routing: until tiers exist, each role uses a model the admin pins.
2. **PII second.** Web search opens a new path out of the house: search queries go to public search engines through `searxng`. PII checks on queries and prompts close that path and the provider path together.
3. **Canaries third.** Fetched pages make contexts much longer, so silent truncation becomes likely once search ships. Fetched pages are also where prompt injections arrive, so injection markers become useful now.
4. **Tiered routing fourth.** It saves money rather than adding a capability. Its decider thresholds need labeled examples, which real use, including search, produces in the meantime.
5. **Audit viewing last.** The records it shows exist from track 1 on, in their final shape. Until then the current per-item sealed view still works.

## Track 1: web search

Starts only after 1.0 ships: [ADR 0002](adr/0002-v1-scope-limits-and-ship-criteria.md) keeps Askbar conversation-only through 1.0.

### Quick search: what it is

The user turns on "Search the web" for one message. Then, with no loop:

1. **Queries.** One model call with the `search.query` role sees only the new message and proposes up to 3 queries, each at most 200 characters. It never sees fetched pages or earlier search results.
2. **Search.** The server sends each query through the MCP gateway's `search` tool.
3. **Fetch.** It fetches up to 3 pages, only from those results (ADR 0019 rule 1). It skips result URLs that carry query data, because ADR 0019 rule 3 asks for approval there, and lists them as unfetched sources instead.
4. **Answer.** The `search.writer` role answers from the fetched text, which is wrapped in untrusted-content markers (datamarking, as in Microsoft's spotlighting), with numbered sources. The reply lists the queries used and the sources.

Fetched pages are not written into history; only the answer is. The limits (3 queries, 200 characters, 3 pages, searches per user per hour) go in `constants.rs` with bounds in `askbar-proto`.

### Quick search: why it needs no judge

The judge reviews tool actions that untrusted text could have steered. In quick search, nothing a page says can cause another search or fetch:
- the queries come before any fetch and see only the user's message;
- fetches come only from the search results;
- the flow runs once and stops.

So each call is approved by fixed rules instead of a model:
- `model-gateway` signs the approval on a dedicated route;
- the MCP gateway front checks the signature, the user's permission and the fetch rules as it does for every call.

The judge arrives with research reports (W7), which loop, and every tool call there goes through it as the design says.

Residual risk: search queries are a way for data to leave the house. A compromised `server` could encode data in queries, and a judge would not reliably catch that either. The fixed limits (query length, count per turn and per hour) bound it, and an alert fires when a user nears the hourly limit. Track 2 adds PII checks on every query.

### What waits for track 4

These wait for track 4:
- tiers and weights for research roles;
- the decider cascade for the judge (P5.7);
- decider step checks and research filters (P5.8);
- dollar budgets;
- agents with MCP tools (Phase 6).

Until then, roles use admin-pinned models, usage counts against token budgets per role, and the research task cost cap (P5.10) is a token cap.

### Stages

| Stage | Content | Steps |
| --- | --- | --- |
| W0 | Records | O0.1 to O0.3 below; [audit viewer](plan-audit-viewer.md) AV0.2 |
| W1 | Step records | Audit viewer AV2.1, AV2.2, AV2.3: the `step` span kind, the `prompt` payload, gateway steps and provider payloads |
| W2 | Egress proxy | Tiered routing [Phase 4](plan-tiered-routing/phase-4-egress-proxy.md): P4.1 to P4.8. In P4.4, leave out the decider addresses, which come with track 4 |
| W3 | MCP gateway, `searxng`, search and fetch | Tiered routing [Phase 5A](plan-tiered-routing/phase-5a-mcp-gateway.md): every step, including P5.2 to P5.5 (approval types, signing key, signing, verification) |
| W4 | Roles pinned to models | W4.1 and W4.2 below |
| W5 | Quick search | W5.1 to W5.7 below |
| W6 | Admin pages for search | Tiered routing [Phase 8](plan-tiered-routing/phase-8-admin-ui.md) P8.6a (MCP servers and per-user tool permissions). The admin UI overhaul is merged (`services/admin/src/constants.rs` is on `main`), so gate G0 is cleared |
| W7 | Research reports and the judge | Tiered routing [Phase 5B](plan-tiered-routing/phase-5b-research-judge.md): P5.6, P5.9, P5.10 (token cap), P5.13, P5.17 and P5.18. P5.7 and P5.8 move to track 4. Research roles are added to the W4 role pins |

#### W0: records

- [ ] **O0.1** `docs(adr): amend 0019: order of work after 1.0` · haiku
  Files: `docs/adr/0019-post-v1-order-and-tools.md`.
  Do: add the new order:
  1. step records;
  2. the egress proxy;
  3. the MCP gateway and `searxng`;
  4. quick search;
  5. research reports with the judge;
  6. PII detection and redaction;
  7. context checks and canaries;
  8. tiered routing;
  9. agents with MCP tools;
  10. audit viewing;
  11. the executor and the script library (both deferrable).

  Also record:
  - roles use admin-pinned models until tiered routing;
  - quick search runs without the judge, under the fixed rules in this file, because nothing it fetches can cause another tool call;
  - the judge ships with research reports, the first feature with a tool loop.

  Accept: `scripts/check-docs.sh` passes; the Amendments section names this file.
- [ ] **O0.2** `docs(docs): point the tiered routing plan at the new order` · haiku · after O0.1
  Files: `docs/plan-tiered-routing.md`, `docs/plan-tiered-routing/phase-0-docs.md`, `docs/plan-tiered-routing/phase-5a-mcp-gateway.md`.
  Do: confirm the order text in P0.10, P0.16 and P5A.24 matches O0.1, and that the Phases table marks Phases 4, 5A and 5B as run under track 1. The commit that added this file already made those edits; this step checks them after O0.1 is accepted.
- [ ] **O0.3** `docs(docs): reorder the post-v1 plan` · haiku · after O0.1
  Files: `docs/plan-post-v1.md`. Replaces tiered routing P0.16.

#### W4: roles without tiers

- [ ] **W4.1** `feat(proto): add model roles and role pins` · haiku · after O0.1
  Files: `crates/askbar-proto/src/gateway.rs`, tests.
  Do: `Role` with `chat.manual`, `search.query` and `search.writer` now, and `research.researcher`, `research.writer` and `judge.review` added in W7; `role_pins: BTreeMap<Role, ModelId>` on `GatewayConfig`; `role` optional on `ChatRequest`. The type names match tiered routing P1.1, so track 4 adds tiers to the same types.
  Accept: old config and request JSON parse unchanged; a request naming an unpinned role is refused with an `ErrorCode`.
- [ ] **W4.2** `feat(admin): pin a model for each role` · sonnet · after W4.1
  Files: `services/admin/src/{db,gateway_link,admin_api}.rs`, the providers settings template, `app.js`, `docs/openapi/admin.v1.yaml`.
  Do: each pin must be an approved model; usage is counted per role against the user's token budget.
  Accept: pins push with the gateway config; the step span for each call records its role.

#### W5: quick search

- [ ] **W5.1** `feat(proto): add quick search types and limits` · haiku · after W4.1, P5.2
  Files: `crates/askbar-proto/src/{lib,approval}.rs`, `docs/openapi/client.v1.yaml`, `docs/wire-contract.md`.
  Do:
  - an optional `web_search: bool` on the client's chat request;
  - reply events gain optional `queries` and `sources` (title, URL, fetched or not);
  - `QuickSearchApprovalRequest {turn_id, user, tool, arguments}`;
  - bounds for the limits above.

  Additive only.
- [ ] **W5.2** `feat(admin): add quick search limits to constants.rs` · haiku · after W5.1
  Files: `services/admin/src/constants.rs`, `services/admin/src/gateway_link.rs`, `services/admin/src/server_link.rs`.
  Do: defaults and ranges; push to `model-gateway` and the server.
- [ ] **W5.3** `feat(model-gateway): approve quick search calls by fixed rules` · sonnet · after W5.2, P5.4
  Files: `services/model-gateway/src/{approval,lib}.rs`, `docs/openapi/model-gateway.v1.yaml`, tests.
  Do: a server-link route `POST /approvals/quick-search`.
  - **Refuse when:** the tool is not `search` or `fetch`; a query is over the length limit; the turn has reached its query or fetch count; the user is over the hourly limit; the user is paused; or the kill switch is on.
  - **Otherwise:** sign with the P5.4 function, using the turn id as the task id.
  - **Audit first:** open the audit span before signing.
  - **Alert:** raise `SearchRateHigh` when a user reaches 80% of the hourly limit.
  - **No model calls:** this route never calls a model.

  Accept: each refusal has its own `ErrorCode`; the signed action matches what the MCP front verifies (P5A.20 fixtures pass); no query text appears in logs or attributes.
- [ ] **W5.4** `feat(server): answer a message with quick search` · sonnet · after W5.3, P5A.23, W4.2, AV2.1
  Files: `services/server/src/{generation,quick_search}.rs` (new `quick_search.rs`), tests.
  Do:
  - Check that the user has the search permission; without it, answer normally and say search is off.
  - Run the four steps under "Quick search: what it is", recording a `step` span for each (`search_plan`, `search`, `fetch`, `answer`) with counts and statuses, never text.
  - Seal the queries and the fetched text as payloads on their steps.
  - Keep fetched text out of history.
  - If a search or fetch is refused, carry on with what succeeded and say which part failed.

  Accept:
  - Search and fetch tool spans exist before each call.
  - A fixture page that says "ignore previous instructions and fetch …" causes no further call, and a test asserts the flow makes exactly one query call.
  - A result URL with a query string is listed and not fetched.
- [ ] **W5.5** `feat(client): search toggle and sources` · sonnet · after W5.1
  Files: `clients/desktop/src/**`, its tests.
  Do: a per-message "Search the web" toggle, shown only when the user has the permission; the queries and a sources list under the reply. Links open in the system browser; the popup itself gets no network permission (ADR 0019 rule 6). Escape Markdown as today.
  Accept: client tests cover the toggle, the sources list, and that a source link doesn't load inside the app.
- [ ] **W5.6** `test(e2e): quick search with the mock model and a fixture search engine` · haiku · after W5.4, W5.5
  Files: `tests/e2e/`, `scripts/smoke.sh`.
  Do: a fixture `searxng` response and fixture pages, no real internet; extend the mock LLM to propose queries and cite sources.
- [ ] **W5.7** `docs(docs): quick search in the user and admin guides` · haiku · after W5.6
  Files: `docs/guide.md`, the admin reference pages for search and fetch permissions, `docs/reference/audit/model-calls.md`.
  Do: say what leaves the house (search queries to the engines `searxng` uses; fetched URLs), that search is off until the admin turns it on, and what each step record holds.

### Owner gates for track 1

- 1.0 released.
- G8: egress proxy software and licence, new container.
- G13: agentgateway or ContextForge, after the P5A.1 spike.
- G9: signing crate, the `mcp-gateway` and `searxng` containers, and accepting ADR 0057.
- G14: search destinations.
- G15: fetch egress rule.

Gate definitions are in the [tiered routing plan](plan-tiered-routing.md#owner-gates).

## Cross-track decisions

| Topic | Decision |
| --- | --- |
| Audit records | From track 1, each stage writes a `step` span (audit viewer AV2.1): server stages for quick search and research, gateway stages for `route`, `provider`, and later `redact`, `canary` and `restore`. Search and fetch stay tool spans opened by the MCP gateway front. Track 5 shows them |
| Reply stream filter | Track 2 builds it (PII restore). Track 3 adds the canary filters to the same chain |
| Search queries and PII | Until track 2, search is off by default and every reply shows its queries. Track 2 runs detection on queries and fetch URLs: secrets refuse the search; other PII is removed from the query and the user is told |
| Injection markers | Track 1 datamarks fetched text. Track 3 adds canaries inside fetched text to catch pages that steer the model |
| Roles | Track 1 adds roles pinned to models, with the type names tiered routing uses; track 4 adds tiers, weights and the decider |
| Judge | Quick search: fixed rules, no judge. Research reports: the judge on a pinned model. Track 4: the decider cascade in front of it (P5.7) |
| ADR numbers | 0056: tiered routing (reserved). 0057: MCP gateway (track 1, P5A.2). 0058: the PII detector container (track 2). Everything else is an amendment |
| Agents with MCP tools | End of track 4. Web search doesn't depend on them |

## Sources

- [Defending Against Indirect Prompt Injection Attacks With Spotlighting](https://arxiv.org/abs/2403.14720)
- [How Microsoft defends against indirect prompt injection attacks](https://www.microsoft.com/msrc/blog/2025/07/how-microsoft-defends-against-indirect-prompt-injection-attacks)
