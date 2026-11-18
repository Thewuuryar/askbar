# Phase 8: admin web UI (branch `feat/admin-routing-ui`, deferred)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

May start any time after G0 (G12 is the same gate); numbered after Phase 6, with Phases 9 and 10 (deferrable) behind it. Expand at phase start; follow the admin redesign plan's rules and its test suite (`tests/e2e/admin.test.js`).

- **P8.1** removed: constants live in `constants.rs` from Phase 1 (G0 merges the admin UI first, so nothing is left to move or rebase).
- [ ] **P8.2** `feat(admin): add the routing page` · sonnet · models, tiers, weights, prices, effort, pins, cutoffs, ceilings, caps, training share.
- [ ] **P8.3** `feat(admin): add decider settings` · sonnet · providers, thresholds, zero-retention record.
- [ ] **P8.4** `feat(admin): add the dry-run panel and decider comparison` · sonnet
- [ ] **P8.5** `feat(admin): add example labeling and shared-question review` · sonnet
- [ ] **P8.6** `feat(admin): add spend and subscription limit views` · sonnet
- [ ] **P8.6a** `feat(admin): add MCP server and tool permission pages` · sonnet · after P5A.17 · register servers and credentials (write-only fields), per-user tool permissions, each destination's egress status.
- [ ] **P8.6b** `feat(admin): add agent task views` · sonnet · after P6.17 · task list with status, tool calls used, cost against the cap, steps and decision links; no step content or tool output.
- [ ] **P8.7** `feat(admin): add script review` · sonnet · after P10.4 · only when Phase 10 ships.
- [ ] **P8.8** `test(admin): cover the routing, MCP and task pages at 1280 and 390` · haiku
- [ ] **P8.9** `docs(docs): update the admin guide` · haiku
