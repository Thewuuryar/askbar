# Phase 6: agents with MCP tools (branch `feat/agents`)

Previous: [Phase 5B](phase-5b-research-judge.md) · Next: [Phase 8](phase-8-admin-ui.md) · Tracker: [task-tracker.md](../task-tracker.md)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

Agents use MCP tools only; there is no executor and no G10 (the executor is Phase 9). Needs Phase 5A, Phase 5B and G0. It reuses Phase 5B's task tables (P5.9), tool-call limit and cost cap (P5.10), `/decide` (P5.6, P5.8) and role table (P5.14); do not rebuild them. Every tool call goes `server` to `/decide` (use `judge`) to a signed approval to the MCP gateway front; tool output is untrusted data and never reaches the judge. New behaviour values (maximum steps per plan, the fixer round limit) go in `constants.rs` with bounds in `askbar-proto`.

- [ ] **P6.1** `feat(proto): add plan and step types` · haiku · after P5.2
  Files: `crates/askbar-proto/src/task.rs`, tests.
  Do: `Plan {steps}`, `PlanStep {id, instructions, inputs, tool: Option<String>, check}` with at most one allowed MCP tool or none, and `Check` as `schema {json_schema}`, `decider` or `planner`; the planner's output JSON schema; additive, `#[serde(default)]`.
  Accept: round-trip tests; a step with two tools does not parse; old task payloads still parse.
- [ ] **P6.2** `feat(admin): add agent values to constants.rs and push them to the server` · sonnet · after P6.1, P5.15, G0
  Files: `crates/askbar-proto/src/task.rs`, `services/admin/src/{constants,server_link}.rs`, `services/server/src/snapshot.rs`, tests.
  Do: `MIN_`/`MAX_`/`DEFAULT_` for the maximum steps per plan, the fixer round limit (1: a second fixer round ends the task), the largest tool output kept in a worker's context; the tool-call and cost-cap values from P5.10 are reused. Pushed in the server snapshot, additive and defaulted, rejected outside the bounds.
  Accept: a changed value changes the snapshot version; an out-of-bounds value is rejected and the old one stays; an old snapshot parses.
- [ ] **P6.3** `feat(server): read the user's permitted MCP tools through the front` · sonnet · after P5A.11, P5A.17, P6.1
  Files: `services/server/src/mcp.rs` (new), `docs/openapi/mcp-gateway.v1.yaml` (client side), tests with the mock MCP front.
  Do: at task start the server asks the MCP gateway front for the user's permitted tools (names and argument schemas); the answer is the only tool list the planner sees; tool descriptions are untrusted text (length-capped, never treated as instructions); an unreachable front means a task with no tools, recorded.
  Accept: a user with no permission gets an empty list; the list is read per task, not cached across users.
- [ ] **P6.4** `feat(server): ask the planner for a plan limited to the permitted tools` · sonnet · after P6.3, P5.14
  Files: `services/server/src/agent/{planner,mod}.rs` (new), tests.
  Do: one `agent.planner` call through `model-gateway` (`role`, `task_id`) with the user's request and the permitted tool names and schemas; a fixed prompt asks for the P6.1 plan JSON and nothing else; the step row for the plan is written before the call.
  Accept: a scripted mock plan parses; a reply that is not the plan format is an invalid plan (P6.5).
- [ ] **P6.5** `feat(server): validate the plan before any step runs` · sonnet · after P6.4, P5.10
  Files: `services/server/src/agent/validate.rs` (new), tests.
  Do: refuse a plan if a tool does not exist, is not in the user's permitted list, a step names more than one tool, the step count passes the maximum, the plan's possible tool calls pass the task tool-call limit (P5.10), a `schema` check has an invalid JSON schema, or a `decider` check is chosen while no decider is set for `step_check` (it becomes `planner`); A13 applies to a refused plan.
  Accept: table tests per rule; a second invalid plan stops the task with its error codes recorded.
- [ ] **P6.6** `feat(server): run a worker step with its step context only` · sonnet · after P6.5, P5.9
  Files: `services/server/src/agent/worker.rs` (new), tests.
  Do: write the step's `task_steps` row before it starts (P5.9); one `agent.worker` call with the step's instructions, inputs, the outputs it needs from earlier steps and the one allowed tool's schema, and nothing else; the reply is a tool call request or a final output; loop until a final output; each attempt is its own span linked by `task_id` and `decision_id`.
  Accept: a test inspects the request body: no other step's text, no plan, no other tool; the row exists before the call.
- [ ] **P6.7** `feat(server): send each tool call through the judge, a signed approval and the MCP front` · sonnet · after P6.6, P5.6, P5A.11, P5.10
  Files: `services/server/src/agent/tool_call.rs` (new), tests.
  Do: refuse a call to a tool other than the step's allowed tool; apply ADR 0019's fixed rules; check and count the tool-call limit before the call; `POST /decide` with use `judge`, the tool, exact arguments and destination, the user's original request and the plan step, never fetched pages or tool output; on approval call the MCP front with the signed approval; on a refusal or a missing approval the call does not happen and the worker gets only the reason code; the tool result goes to the worker as a delimited block of untrusted data, cut to the context limit from P6.2.
  Accept: with a mock `/decide` and a mock front: approved call runs once; refusal runs nothing; an unplanned tool is refused before `/decide`; the judge input in the request body has no tool output.
- [ ] **P6.8** `feat(server): check steps against a JSON schema` · sonnet · after P6.6
  Files: `services/server/src/agent/check.rs` (new), tests.
  Do: validate the step's final JSON output against the step's schema (A13); if no JSON schema crate is in `Cargo.lock`, stop and ask before adding one (new dependency; `cargo deny`).
  Accept: valid passes, invalid and non-JSON fail with distinct codes.
- [ ] **P6.9** `feat(server): review a step with the planner when no deterministic check applies` · sonnet · after P6.8
  Files: `services/server/src/agent/check.rs`, tests.
  Do: a `planner` check, and the fallback when a decider check is below `pass`, unset or errors, calls `agent.planner` with the step's acceptance text and output in a fixed pass or fail format; any other reply is a fail.
  Accept: scripted pass, fail and off-format replies behave as stated.
- [ ] **P6.10** `feat(server): check steps with the decider's pass answer` · sonnet · after P6.9, P5.8
  Files: `services/server/src/agent/check.rs`, tests.
  Do: for a `decider` check call `/decide` with use `step_check`; at or above `pass` the step is done, below it the planner reviews (P6.9); a decider error goes to the planner; live only when `step_check` mode is `live` (shadow first, P6.18); the decider never sees tool output beyond the step's output text.
  Accept: tests for at, above and below the threshold, decider error and shadow mode (planner decides).
- [ ] **P6.11** `feat(server): retry once on standard, then the fixer` · sonnet · after P6.9, P6.2
  Files: `services/server/src/agent/{retry,fixer}.rs` (new), tests.
  Do: a failed check reruns the step once on `standard` (`worker_retry` in `reasons`); a second failure calls `agent.fixer`, which returns revised steps that go through P6.5 validation; a second fixer round on one step ends the task as `stopped` with results saved; every attempt is a row.
  Accept: scripted failures: one retry then pass; retry then fixer then pass; second fixer round stops the task; revised steps with a new tool are refused.
- [ ] **P6.12** `feat(server): stop at the tool-call limit or cost cap and answer from finished work` · sonnet · after P6.7, P6.11, P5.10
  Files: `services/server/src/agent/{finish,mod}.rs`, tests.
  Do: use P5.10's checks (tool-call limit; cost cap with `problem+json` 402 and alerts at 75% and 90%); when either stops the task the planner answers from finished steps only; a finished plan ends with the planner's final answer; status `done` or `stopped`; tiers do not change mid-task.
  Accept: limit and cap tests stop mid-plan with saved results and a planner answer; a normal run ends `done`.
- [ ] **P6.13** `feat(server): mark provider failures interrupted and resume` · sonnet · after P6.11, P5.9
  Files: `services/server/src/agent/resume.rs` (new), `store.rs`, tests.
  Do: when every in-tier fallback try fails the task ends `interrupted`; Resume continues from the next unfinished step; a step whose row started but never ended reruns only after the user confirms (its tool calls may have happened); the start-up sweep that marks `running` tasks `interrupted` (P5.9) is reused.
  Accept: tests: resume skips finished steps; a cut-off step without confirmation does not run; with confirmation it reruns once; a restart marks a running task interrupted.
- [ ] **P6.14** `feat(server): add the client API to start an agent task and stream progress` · sonnet · after P6.12
  Files: `services/server/src/*` (routes), `docs/openapi/client.v1.yaml`, `docs/wire-contract.md`, tests.
  Do: follow "Adding an endpoint, end to end": start a task for a conversation, stream progress (plan, step started, tool call refused or run, check result, retry, done) as SSE events with no tool output text, read a task's state; 402 `problem+json` for the cap; additive.
  Accept: OpenAPI test; old clients ignore the new events; a task of another user is 404.
- [ ] **P6.15** `feat(server): add the client API to resume and cancel a task` · sonnet · after P6.13, P6.14
  Files: same as P6.14.
  Do: Resume with an optional `confirm_rerun` for the cut-off step; cancel stops the loop after the current call, keeps results and marks the task `stopped`.
  Accept: OpenAPI test; resume without confirmation of a cut-off step is refused with a clear error; cancel is idempotent.
- [ ] **P6.16** `feat(model-gateway): add planner, worker and fixer role defaults` · haiku · after P5.14
  Files: `services/model-gateway/src/routing.rs`, tests.
  Do: `agent.planner` `frontier`; `agent.worker` `fast` with `retry_tier` `standard`; `agent.fixer` `frontier`; a pin replaces the tier, with a backup pin.
  Accept: tests per role; a pin to an unapproved model rejects the push.
- [ ] **P6.17** `feat(admin): set pins for the planner, worker and fixer` · sonnet · after P6.16, P5.15, G0
  Files: `services/admin/src/{routing,gateway_link}.rs`, OpenAPI, tests.
  Do: API to read and set the three roles' tiers and pins within the `constants.rs` ranges; push to the gateway; warn when the judge's pinned provider equals the planner's or worker's.
  Accept: a pin to an unapproved model is refused before pushing; the warning appears.
- [ ] **P6.18** `feat(model-gateway): shadow decider step checks against the planner` · sonnet · after P5.8, P6.10
  Files: `services/model-gateway/src/{decide,decider}.rs`, tests.
  Do: while `step_check` mode is `shadow` the planner's verdict decides and the decider's `pass` probability is recorded beside it in the decision record (probabilities only); the decider call opens its own span first and counts against the task cap.
  Accept: no step outcome changes in shadow; both answers are on the record; a decider failure records `decider_error`.
- [ ] **P6.19** `feat(admin): report step-check shadow results` · sonnet · after P6.18, P1.6, G0
  Files: `services/admin/src/routing.rs`, OpenAPI, tests.
  Do: `GET /api/admin/v1/routing/step-check-report`: per decider, the share of steps where its pass or fail agreed with the planner, errors and timeouts, from decision records only; suggested `pass` threshold.
  Accept: fixture records give expected numbers; the owner reads this before flipping `step_check` live.
- [ ] **P6.20** `feat(client): show agent tasks and progress` · sonnet · after P6.14
  Files: `clients/desktop/ui/*`, `clients/desktop/src/*`.
  Do: a task view in the conversation: plan steps with status, tool calls used against the limit, cost against the cap, retries; text via `textContent`; follow `docs/style/ui-ux.md`.
  Accept: client checks and `npm test`.
- [ ] **P6.21** `feat(client): add Resume, Cancel and the rerun confirmation` · sonnet · after P6.15, P6.20
  Files: `clients/desktop/ui/*`, `clients/desktop/src/*`.
  Do: Resume and Cancel on an interrupted or running task; a dialog before rerunning a cut-off step that says its tool calls may already have happened.
  Accept: UI tests for resume, cancel and both answers to the dialog.
- [ ] **P6.22** `test(e2e): add a mock MCP tool server` · haiku · after P5A.11
  Files: `tests/e2e/mock_mcp.py` (new), `compose.yaml` `dev` profile entry if needed (stop-and-ask for a container).
  Do: an MCP tool server with tools `echo`, `search` and `fetch` and keywords: slow, 500, oversize, and output that carries hostile text.
  Accept: usable by P6.24 and P6.25.
- [ ] **P6.23** `test(e2e): script mock LLM plans` · haiku · after P6.1
  Files: `tests/e2e/mock_llm.py`.
  Do: keyword-selected scripted planner plans (valid, invalid tool, too many steps), worker replies (tool call, final output, bad JSON) and fixer revisions.
  Accept: usable by P6.24 and P6.25.
- [ ] **P6.24** `test(server): run agent tasks against the mock tool server and scripted plans` · sonnet · after P6.15, P6.22, P6.23
  Files: `services/server/tests/agents.rs`.
  Do: a two-step plan with a tool call (approved, signed, run once); schema check pass and fail; retry then fixer; second fixer round ends the task; tool-call limit and cost cap stops; provider failure to `interrupted`, Resume, and the rerun confirmation.
  Accept: every case ends in the stated task status with saved results.
- [ ] **P6.25** `test(server): tool output that asks for other tools or settings is refused` · sonnet · after P6.24
  Files: `services/server/tests/agents_injection.rs`.
  Do: ADR 0016 "Defend": the mock tool returns text telling the worker to call another tool, call a tool the user lacks, approve itself, or change settings; the scripted worker obeys it.
  Accept: the call to a tool outside the step is refused before `/decide`; a call to a tool the user lacks is refused by plan validation or the front; a mock judge refuses the crafted action; no setting changes; the judge request body holds no tool output.
- [ ] **P6.26** `test(server): agent task smoke` · haiku · after P6.24
  Files: `scripts/smoke.sh`.
  Do: smoke step with the mock LLM and mock tool server: start a task, watch progress, finish `done`.
  Accept: `scripts/smoke.sh` passes.
- [ ] **P6.27** `docs(adr): amend 0019: agents use MCP tools, executor deferred` · haiku · after P6.12. Files: `0019-post-v1-order-and-tools.md`. Decision.
- [ ] **P6.28** `docs(adr): amend 0054: agent roles and step checks` · haiku · after P6.18. Files: `0054-model-gateway.md`.
- [ ] **P6.29** `docs(adr): amend 0039: the agent loop and task API` · haiku · after P6.15. Files: `0039-server.md`.
- [ ] **P6.30** `docs(adr): amend 0040: agent role pins and step-check report` · haiku · after P6.19. Files: `0040-admin.md`.
- [ ] **P6.31** `docs(adr): amend 0044: agent tasks, Resume and rerun confirmation` · haiku · after P6.21. Files: `0044-desktop-client.md`.

Phase end: full checks, PR. The owner reads the step-check report before `step_check` goes live.
