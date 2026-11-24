# Phase 9: executor (branch `feat/executor`, deferrable, coarse)

Previous: [Phase 8](phase-8-admin-ui.md) · Next: [Phase 10](phase-10-script-library.md) · Tracker: [task-tracker.md](../task-tracker.md)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

Deferrable: starts only when the owner asks, after Phase 8. Expand at phase start. Needs Phase 6, G10. The executor runs untrusted code; search and fetch stay on the MCP gateway. It verifies approvals with P5.5's code, has no `model-gateway` link and no secrets.

- [ ] **P9.1** `feat(proto): add executor work and output types` · haiku
- [ ] **P9.2** `feat(executor): add the service, pairing and approval check` · sonnet · public key at pairing, no gateway link, no secrets; G10.
- [ ] **P9.3** `feat(compose): add executor-link` · sonnet · optional Tailscale; `check-isolation.sh`.
- [ ] **P9.4** `feat(server): add code and file tools that run in the executor` · sonnet · planner tools beside the MCP tools; each call through the judge and a signed approval.
- [ ] **P9.5** `feat(server): check steps by exit code and tests` · sonnet · deterministic checks beside the schema check (P6.8).
- [ ] **P9.6** `docs(adr): amend 0020, 0054, 0039, 0040 for the executor` · haiku · one commit each; 0020 records the executor as built after the MCP gateway.
