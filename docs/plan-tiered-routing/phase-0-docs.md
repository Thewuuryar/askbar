# Phase 0: records and docs (branch `docs/tiered-routing-adr`)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

Docs only. Ends with G1.

- [x] **P0.1** `docs(docs): add the tiered routing design` · haiku · after none
  Files: `docs/design-tiered-routing.md`.
  Do: copy the design file the owner attached (stop and ask if it is missing); make ADR and repo links relative. (The design already uses ADR 0056 and ADR 0001 rules 1 and 2.)
  Accept: the new ADR is cited as 0056 throughout; `scripts/check-docs.sh` passes.
- [x] **P0.2** `docs(docs): replace the tiered routing plan` · haiku · after P0.1
  Files: `docs/plan-tiered-routing.md`, any file linking to it (`grep -rn plan-tiered-routing`).
  Do: overwrite with the attached plan; fix links.
  Accept: docs check passes. From here on tick boxes in the repo copy.
- [ ] **P0.3** `docs(adr): propose 0056 tiered model routing` · sonnet · after P0.1
  Files: `docs/adr/0056-tiered-model-routing.md` (from `0000-template.md`), `docs/adr/README.md` index row.
  Do: Status Proposed. Decision, reasons and consequences from the design (tiers, roles, Jev as a decision provider, signed approvals, script library, rollout, budgets in dollars). Link the amendments below. Do not write "Accepted".
  Accept: ADR format check passes; README row added. Gate: owner accepts (G1).
- [ ] **P0.4** `docs(adr): amend 0014: tiers, deciders, dollar budgets, fallback` · haiku · after P0.3
  Files: `docs/adr/0014-model-gateway-and-providers.md`.
  Do: use the design's ADR table row (catalog tiers and weights, default tier, role pins, deciders, prices and effort, dollars, `auto`, in-tier fallback, an approved decider may see prompts). Add an Amendments line.
  Accept: docs check. Decision amendment: owner approves in PR.
- [ ] **P0.5** `docs(adr): amend 0002: routing after 1.0 and latency budgets` · haiku · after P0.3. Files: `0002-v1-scope-limits-and-ship-criteria.md`. Design row. Decision.
- [ ] **P0.6** `docs(adr): amend 0010: decision records and task spans` · haiku · after P0.3. Files: `0010-auditor.md`. Design row. Decision.
- [ ] **P0.7** `docs(adr): amend 0011: shared questions and hosted decider` · haiku · after P0.3. Files: `0011-sealed-content-access.md`. Two exceptions, each with notice and a record per use. Decision.
- [ ] **P0.8** `docs(adr): amend 0012: notices for sharing and the decider` · haiku · after P0.3. Files: `0012-consent-notices-and-deletion.md`. Decision.
- [ ] **P0.9** `docs(adr): amend 0016: routing and library risks` · haiku · after P0.3. Files: `0016-risk-register.md`. Defend and Accept lists from the design row. Register.
- [ ] **P0.10** `docs(adr): amend 0019: order, judge role, writer and pins` · haiku · after P0.3. Files: `0019-post-v1-order-and-tools.md`. Order: routing, egress proxy, MCP gateway, research and judge, agents with MCP tools, admin UI, executor (deferrable), library (deferrable); hosted Jev on the allowlist if approved. Decision.
- [ ] **P0.11** `docs(adr): amend 0020: approvals, library scripts and the executor deferred` · haiku · after P0.3. Files: `0020-remote-executor-and-ca-triggers.md`. The executor comes after agents with MCP tools and is deferrable. Decision.
- [ ] **P0.12** `docs(adr): amend 0009: task state in the space file` · haiku · after P0.3. Files: `0009-spaces-and-storage.md`. Decision.
- [ ] **P0.13** `docs(adr): amend 0022: the judge is a role, not a caller` · haiku · after P0.3
  Files: `0022-api-path-conventions.md`.
  Do: first `grep -n judge` in 0022; if it lists the judge as a `model-gateway` caller, correct it (the server calls the judge role). If it does not, tick as "not needed" with no commit.
  Accept: matches the architecture edit in P0.14. Decision if it changes a rule.
- [ ] **P0.14** `docs(docs): add tiered routing terms to the architecture` · haiku · after P0.3
  Files: `docs/architecture.md`.
  Do: model-gateway row (line 17); reserved names (line 21): drop judge and add the one-line note, remove it as a gateway caller; glossary (line 23): the design's term table, space changed.
  Accept: docs check; glossary names match the design.
- [ ] **P0.15** `docs(docs): describe Auto, tiers and /decide in the architecture` · haiku · after P0.14
  Files: `docs/architecture.md`.
  Do: "Model choice" (~line 234) gains Auto, tier, effort; budgets in dollars; Links table gains `/decide`. Mark each as post-1.0.
  Accept: docs check.
- [ ] **P0.16** `docs(docs): reorder post-v1 plan for tiered routing` · haiku · after P0.3
  Files: `docs/plan-post-v1.md` (status line 3, V2 sketch lines 5 to 7, mermaid lines 9 to 59).
  Do: new order; the design's V2 sentence; replace the diagram with the design's.
  Accept: mermaid parses (docs check).
- [ ] **P0.17** no commit: update the claude.ai project doc `architecture-goals-and-features.md` · owner · after G1
  Do: the orchestrator has no access to the claude.ai project; the owner asks a claude.ai chat in the "Askbar AI" project to apply the design's four bullets (Clients, Server, Admin, Later), then ticks this box.
  Accept: only after the ADR is accepted.

Phase end: open the PR, mark every decision amendment for approval, stop at G1.
