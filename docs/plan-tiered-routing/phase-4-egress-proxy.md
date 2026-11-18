# Phase 4: egress proxy (branch `feat/egress-proxy`, coarse)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

When the phase starts, a Sonnet planning subagent expands each line into detailed steps (files, accept, agent) and the orchestrator commits that expansion to this file first (`docs(docs): expand egress proxy steps`). Needs G8.

- [ ] **P4.1** `docs(adr): confirm ADR coverage for the egress proxy` · haiku · check ADR 0019 and 0038 name it; if not, the owner decides on a new ADR.
- [ ] **P4.2** `feat(compose): add the egress proxy service` · sonnet · new container, networks; `check-isolation.sh`.
- [ ] **P4.3** `feat(compose): send gateway, cpa, alerter and searxng traffic through the proxy` · sonnet
- [ ] **P4.4** `feat(admin): push per-source allowlists to the proxy` · sonnet · hosted decider addresses if approved; the self-hosted decider host as an approved private address.
- [ ] **P4.5** `test(compose): prove default deny and private-address blocking` · sonnet · extend isolation checks.
- [ ] **P4.6** `docs(adr): amend 0006: egress through the proxy` · haiku
- [ ] **P4.7** `docs(adr): amend 0032: proxy container limits` · haiku
- [ ] **P4.8** `docs(docs): update the architecture network table and diagram` · haiku
