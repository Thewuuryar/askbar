# Phase 8: admin web UI (branch `feat/admin-routing-ui`, deferred)

Previous: [Phase 6](phase-6-agents.md) · Next: [Phase 9](phase-9-executor.md) · Tracker: [task-tracker.md](../task-tracker.md)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

May start any time after G0 (G12 is the same gate); numbered after Phase 6, with Phases 9 and 10 (deferrable) behind it. Expand at phase start; follow the admin redesign plan's rules and its test suite (`tests/e2e/admin.test.js`).

- **P8.1** removed: constants live in `constants.rs` from Phase 1 (G0 merges the admin UI first, so nothing is left to move or rebase).
- [ ] **P8.2** `feat(admin): add the routing page` · sonnet · models, tiers, weights, prices, effort, pins, cutoffs, ceilings, caps, training share.
- [ ] **P8.3** `feat(admin): add decider settings` · sonnet · providers, thresholds, zero-retention record.
- [ ] **P8.4** `feat(admin): add the dry-run panel and decider comparison` · sonnet
- [ ] **P8.5** `feat(admin): add example labeling and shared-question review` · sonnet
- [ ] **P8.6** `feat(admin): add spend and subscription limit views` · sonnet
#### W6: admin pages for search (track 1)

Track 1 runs P8.6a, P8.6c to P8.6f, P8.8a and P8.9a as stage W6 of [plan-priorities.md](../plan-priorities.md). Every page writes host names, errors and other values from a service with `textContent` only. Each change is an audited admin action, refused if its record cannot be appended. Server registry pages (P8.6g) wait for P5A.16 and are not part of W6.

- [x] **P8.6a** `feat(admin): add the MCP page` · sonnet · after P5A.18, P5A.19
  Files: `services/admin/src/{mcp,mcp_link,approval_key,egress_link,routes}.rs`, a new `mcp.html` template, `services/admin/src/ui/*.js`, `docs/openapi/admin.v1.yaml`, `services/admin/tests/{mcp,mcp_link,openapi}.rs`.
  Do:
  - **Tool permissions:** a table of users with a `search` and a `fetch` switch each, over the existing `/mcp/permissions` routes. Default off, as P5A.17 stores it.
  - **Front status:** a new session-only `GET /api/admin/v1/mcp/status` returns what `McpLink` already keeps: online, last error, the pushed and the reported config version, and the front's call counts by outcome. A front that is not configured shows a banner saying it refuses every call.
  - **Approval key:** show the key's fingerprint and version. Add `POST /api/admin/v1/mcp/approval-key/rotate`. It is session-only and needs a sign-in within 10 minutes, as backup key rotation does. It calls `approval_key::rotate` and pushes the new public key to the front and to `model-gateway` at once.
  - **Egress status:** a new session-only `GET /api/admin/v1/egress/status` returns, per source, the entries the admin pushed and the proxy's allowed and denied counts from its last `/status`. The MCP page shows the `searxng` and `fetch` rows from it.
  Accept: admin tests against a fake front and a fake proxy cover status, rotation (the new key is pushed; a call signed with the old key is refused by the fake front after the push) and the egress rows; an API key gets 401 on the session-only routes; no key material in any response or log; OpenAPI test passes.
- [x] **P8.6c** `feat(admin): add the egress page` · sonnet · after P8.6a, P4.4a, P4.4b
  Files: a new `egress.html` template, `services/admin/src/ui/*.js`, `services/admin/src/{egress_link,services,routes}.rs`, `docs/openapi/admin.v1.yaml`, `services/admin/tests/{egress,services,openapi}.rs`, `docs/reference/settings/services.md`.
  Do:
  - **Approvals:** the page P4.4a deferred to this phase. List, add and remove private destinations (source, host, port, plain HTTP, note) over the existing `/egress/approvals` routes.
  - **Allowlist:** each source's current entries from `GET /egress/status` (P8.6a), each with where it comes from (a provider URL, the alert settings, `CPA_EGRESS_HOSTS`, `SEARXNG_EGRESS_HOSTS`, the `fetch` rule, or an approval). Read only: entries are derived, never typed.
  - **Status:** configured or not, config version, open tunnels, allowed and denied counts per source.
  - **Blocked requests:** move the P4.4b section here and link to it from the Services page.
  - **Restart:** add an `egress-proxy` row to `services.rs` that calls the proxy's `POST /restart`, as the alerter row does.
  Accept: tests for each section against a fake proxy; restart is audited; the e2e approval test adds and removes an approval from the page.
- [x] **P8.6d** `feat(admin): make the quick search and MCP limits editable` · sonnet · after W5.1, P5A.18
  Files: `services/admin/src/{admin_api/settings,constants,gateway_link,server_link,mcp_link}.rs`, a new migration (appended), `docs/openapi/admin.v1.yaml`, `services/admin/tests/{api,gateway,server_link,mcp_link,openapi}.rs`.
  Do:
  - **Which limits:**
    - quick search (`QuickSearchLimits`): queries per reply, query length, pages fetched per reply and searches per user per hour;
    - MCP front: fetch bytes, fetch timeout, redirects, tool call timeout, search results and remembered URLs.
  - **Storage:** an override per limit in the admin's settings table. A limit with no override uses its default from `constants.rs`. Defaults and ranges stay where they are, so the code still has one source for each.
  - **API:** `GET` and `PUT /api/admin/v1/limits` return each limit's value, default and range. `PUT` refuses a value outside its `askbar-proto` range, and `QuickSearchLimits::validate()` checks the whole set. Setting a value to `null` resets it to the default.
  - **Push:** a saved change alters the config version, so `model-gateway`, the server and the front are re-pushed by the links they already have.
  Accept: a changed limit reaches all three fakes within one push interval; an out-of-range value is refused with an `ErrorCode` and nothing is pushed; reset restores the default; the push adds no new fields, since the limits are already in each config; OpenAPI test passes.
- [x] **P8.6e** `feat(admin): turn quick search on and off` · sonnet · after W5.2, P8.6d
  Files: `crates/askbar-proto/src/quick_search.rs`, `services/admin/src/{admin_api/settings,gateway_link,server_link}.rs`, `services/model-gateway/src/approval.rs`, `services/server/src/quick_search.rs`, `docs/openapi/{admin,client}.v1.yaml` if a field is added, tests.
  Do: a global `quick_search_enabled` setting, on by default (per-user permissions still start off), apart from the kill switch. When off, `model-gateway` refuses `POST /approvals/quick-search` with its own `ErrorCode`, and the server answers without search and says search is off, as it does for a user without the permission (W5.3). The server already tells the client whether the user may search (W5.4); that answer becomes false while the setting is off, so the client hides the toggle.
  Accept: with the setting off, a quick search message is answered without any `search` or `fetch` span and the refusal has its `ErrorCode`; turning it on restores search without a restart; additive fields only.
- [x] **P8.6f** `feat(admin): add the search page` · sonnet · after P8.6a, P8.6d, P8.6e
  Files: a new `search.html` template, `services/admin/src/ui/*.js`, `services/admin/src/{routes,cost}.rs`, `services/mcp-gateway/src/{search,status}.rs`, `crates/askbar-proto/src/mcp.rs`, `services/admin/tests/`, `services/mcp-gateway/tests/`, a check in `scripts/check-docs.sh` or a Rust test.
  Do:
  - **SearXNG health:** the front's `GET /status` gains, additively, the time and outcome of its last search request to `searxng`. The page shows it from `GET /mcp/status` (P8.6a), since the admin cannot reach `searxng` itself.
  - **Engines:** show the engines in `docker/searxng/settings.yml` next to `SEARXNG_EGRESS_HOSTS`, read only. A test fails when an enabled engine has no egress host or a host has no engine, so the two cannot drift.
  - **Quick search:** the on/off switch (P8.6e), the limits editor for the four quick search limits (P8.6d), and the role pins for `search.query` and `search.writer` (a link to the W4.2 page).
  - **Usage:** searches, fetches and refusals per day, and tokens and cost per role (`search.query`, `search.writer`) from `token_usage.role`. A new session-only `GET /api/admin/v1/usage/roles` serves the token figures. No query text or URL is shown.
  - **MCP limits:** the editor for the MCP front limits (P8.6d) lives here, under its own heading.
  Accept: admin tests for the usage route against a seeded database; the engine check fails on a fixture with a missing host; a `searxng` that does not answer shows as down within one poll; OpenAPI test passes.
- [ ] **P8.6g** `feat(admin): add MCP server registry pages` · sonnet · after P5A.16 · not part of W6 · register servers and credentials (write-only fields: a credential is never shown again after saving), enable, disable and remove (removal drops the credential and the server's egress entries), each server's egress status from `GET /egress/status`.
- [ ] **P8.6b** `feat(admin): add agent task views` · sonnet · after P6.17 · task list with status, tool calls used, cost against the cap, steps and decision links; no step content or tool output.
- [ ] **P8.7** `feat(admin): add script review` · sonnet · after P10.4 · only when Phase 10 ships.
- [ ] **P8.8** `test(admin): cover the routing and task pages at 1280 and 390` · haiku
- [x] **P8.8a** `test(admin): cover the MCP, egress and search pages at 1280 and 390` · haiku · after P8.6a, P8.6c to P8.6f · W6 · in `tests/e2e/admin.test.js`: each page loads with fixture data, a value with markup shows as text, switching a user's `search` permission, adding and removing an egress approval, editing and resetting a limit, and turning quick search off and on.
- [ ] **P8.9** `docs(docs): update the admin guide` · haiku
- [x] **P8.9a** `docs(docs): the MCP, egress and search pages in the admin guide` · haiku · after P8.8a · W6 · `docs/reference/settings/` pages for each, the limits with their defaults and ranges, approval key rotation, and the quick search switch; amend ADR 0040 (admin) for the new routes and ADR 0056 (egress) for the egress page and restart.
