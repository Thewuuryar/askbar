# Phase 5B: research reports and the judge (branch `feat/research-judge`, coarse)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

Expand into detailed steps at phase start. Needs Phase 5A. Values introduced here (task tool-call default 8 and range 1 to 32, research waits 8 s and 90 s, summary size 800 tokens, decider and judge thresholds) go in `constants.rs`, with bounds in `askbar-proto` where a service checks them (approval expiry is already in Phase 5A); the MCP gateway gets its values through the config push built in P5A.18.

- **P5.1 to P5.5, P5.11, P5.12, P5.16** moved to 5A: P5.1 became P5A.2 (the MCP gateway ADR); P5.2 to P5.5 keep their IDs there; P5.11 is P5A.6 to P5A.12; P5.12 is P5A.13; P5.16 is P5A.20.
- [ ] **P5.6** `feat(model-gateway): add POST /decide for the judge role` · sonnet · fixed input, fixed output format, anything else refuses; pinned model and backup.
- [ ] **P5.7** `feat(model-gateway): run the decider cascade for the judge in shadow` · sonnet · thresholds from labeled tool actions.
- [ ] **P5.8** `feat(model-gateway): add step_check and research decisions to /decide` · sonnet
- [ ] **P5.9** `feat(server): add task tables and mark running tasks interrupted on start` · sonnet · migration; ADR 0009.
- [ ] **P5.10** `feat(server): enforce the tool-call limit and task cost cap` · sonnet · 402 `problem+json`; alerts at 75% and 90%; default and range (8; 1 to 32) from `constants.rs`.
- [ ] **P5.13** `feat(server): run research reports` · sonnet · split, researchers, wait rules (8 s, 90 s), summary size (800 tokens), writer, degrade; values from `constants.rs`.
- [ ] **P5.14** `feat(model-gateway): add role pins and the role table` · sonnet
- [ ] **P5.15** `feat(admin): set roles, decider thresholds and task caps` · sonnet · within the `constants.rs` ranges, pushed with each service's config
- [ ] **P5.17** `docs(adr): amend 0004 and 0049, 0023` · haiku · one commit each: new tokens and volumes and the signing key, new problem types (task cap 402, bad approval). (0028 and 0032 are amended in P5A.25 and P5A.26.)
- [ ] **P5.18** `docs(adr): amend 0054, 0039, 0040 for research and the judge` · haiku · one commit each.
