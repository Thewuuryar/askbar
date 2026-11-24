# Plan: tiered model routing

Status: proposed, after 1.0. **Order:** [plan-priorities.md](plan-priorities.md) runs Phases 4, 5A and 5B (except P5.7 and P5.8) and P8.6a first, as the web search track, with research and judge roles pinned to models; the rest of this plan is track 4, after PII redaction and canaries. Do not start Phase 1 until tracks 1 to 3 have their PRs open. Nothing here is decided until [ADR](adr/README.md) for tiered routing (number assigned when written) and the amendments in Phase 0 are accepted. This file and one file per phase in [plan-tiered-routing/](plan-tiered-routing/) replace the older local-only classification plan of the same name; the boxes in the phase files track progress.

Design: [design-tiered-routing.md](design-tiered-routing.md) (committed in P0.1 from the design file the owner gives the first session). The design wins on behaviour; this plan wins on order. The new ADR has no number until it is written; it takes the next free one (0056 is the egress proxy ADR, 0055 the alerter).

Deferred: every admin web UI task (routing page, decider settings page, dry-run panel, decider comparison, shared-question review, spend views, MCP server and tool permission pages, agent task views) waits for Phase 8, which may start any time after G0 (`claude/trusting-thompson-3vv2sw`, the admin UI overhaul, merged into `main`). Admin backend work from Phase 1 on also waits for G0: new values go straight into `services/admin/src/constants.rs`, which exists only on that branch. Phase 0 (docs) can run before G0. Admin backend API work goes in new modules, without touching `pages.rs`, `web.rs`, `ui/**` or the route table beyond one line per route in `lib.rs`.

Also deferrable: the executor (Phase 9) and the script library (Phase 10, the script review page P8.7 with it) come after Phase 8 and start only when the owner asks. Phases 5A, 5B and 6 ship agents with MCP tools and no executor; the executor and the script library are not needed for them.

## Phases

The orchestrator reads this file and the current phase's file only. It sets a phase to `in progress` in its first step's commit and to `PR open` in its last; the owner sets `done` after merging.

| Phase | File | Status |
| --- | --- | --- |
| 0 Records and docs | [phase-0-docs.md](plan-tiered-routing/phase-0-docs.md) | in progress |
| 1 Records only | [phase-1-records.md](plan-tiered-routing/phase-1-records.md) | not started |
| 2 Shadow | [phase-2-shadow.md](plan-tiered-routing/phase-2-shadow.md) | not started |
| 3 Auto | [phase-3-auto.md](plan-tiered-routing/phase-3-auto.md) | not started |
| 4 Egress proxy | [phase-4-egress-proxy.md](plan-tiered-routing/phase-4-egress-proxy.md) | not started; runs first, as stage W2 of [plan-priorities.md](plan-priorities.md) |
| 5A MCP gateway | [phase-5a-mcp-gateway.md](plan-tiered-routing/phase-5a-mcp-gateway.md) | not started; runs first, as stage W3 of [plan-priorities.md](plan-priorities.md) |
| 5B Research reports and the judge | [phase-5b-research-judge.md](plan-tiered-routing/phase-5b-research-judge.md) | not started; runs in track 1 as stage W7 of [plan-priorities.md](plan-priorities.md), except P5.7 and P5.8 |
| 6 Agents with MCP tools | [phase-6-agents.md](plan-tiered-routing/phase-6-agents.md) | not started |
| 7 | none: the script library moved to Phase 10 | not used |
| 8 Admin web UI (deferred) | [phase-8-admin-ui.md](plan-tiered-routing/phase-8-admin-ui.md) | not started |
| 9 Executor (deferrable) | [phase-9-executor.md](plan-tiered-routing/phase-9-executor.md) | not started |
| 10 Script library (deferrable) | [phase-10-script-library.md](plan-tiered-routing/phase-10-script-library.md) | not started |

Risks and open questions for the owner: [risks.md](plan-tiered-routing/risks.md).

## How to run

Run this plan as [orchestration.md](orchestration.md) says. The orchestrator reads this file and the current phase's file only: the first phase in the Phases table that is not `done` or `PR open`. Start a session with:

> You are the orchestrator for `docs/plan-tiered-routing.md`. Read `docs/orchestration.md`, this file and the current phase file it names, then follow them from the first unchecked step.

This plan adds to the shared rules:

- **Design citations.** When a step cites the design, the brief says: "read only that part. Find it with `grep -n '^##' docs/design-tiered-routing.md` and read that range (for an ADR amendment, the ADR's row in the design's ADR updates table)."
- **Phase state.** One branch per phase, named in the phase heading. "After Phase N" means its status is `PR open` or `done`.
- **ADR amendments.** Each amendment is edited in place with an Amendments line. Decision amendments need the owner's approval in the PR.
- **Budgets.** Token budgets keep working until dollar budgets are switched on.
- **Decider calls.** In the review list, "every model call" includes decider calls.

## Owner gates

| Gate | Needed before | What |
| --- | --- | --- |
| G0 | P1.1a and every admin backend step from Phase 1 on (Phase 0 can run before it) | `claude/trusting-thompson-3vv2sw` (admin UI overhaul, holds `services/admin/src/constants.rs`) merged into `main` |
| G1 | Phase 1 | Accept the tiered routing ADR (Proposed to Accepted); approve the decision-amendment PR (Phase 0) |
| G2 | P2.3 | TypeSafe or Cloudflare (or OpenRouter) access and keys for tests; choose the first decider provider |
| G3 | none | removed: local classifier dropped |
| G4 | hosted decider on | Recorded zero-retention agreement with the hosted decider's provider; privacy notice wording |
| G5 | P2.17 | Self-hosted decider host (OpenJev or Clef), licence (OpenJev weights are CC BY-NC 4.0, so commercial use needs a commercial licence; Clef and Clef-flash: check licence), network route |
| G6 | none | removed: local classifier dropped |
| G7 | Phase 3 go-live | Shadow results meet the bar (under 5% frontier-to-fast misses, cheaper than always-`standard`); owner flips `chat_auto.mode` to live and approves Auto as default |
| G8 | Phase 4 | Egress proxy software and licence; new container; compose and network changes |
| G9 | Phase 5A (P5.3 on) | Ed25519 signing crate; MCP gateway software: agentgateway (Apache-2.0 image behind a Rust front) or, as fallback, IBM ContextForge, as chosen at G13; new containers `mcp-gateway`, `searxng`; accept the MCP gateway ADR; where approval verification code lives (ADR 0045 allows, or a new crate) |
| G10 | Phase 9 (deferrable) | Executor host and hardware; new image |
| G11 | Phase 10 (deferrable) | Owner promotes each script; go-live bar met |
| G12 | Phase 8 | same as G0; Phase 8 may start any time after it |
| G13 | P5A.2 | Owner reads the spike result (P5A.1) and picks agentgateway or ContextForge (the unverified items decide) |
| G14 | P5A.14 | Search MCP server and its destinations: searxng and the upstream search engines it reaches through the egress proxy |
| G15 | P5A.15 | Fetch MCP server and its egress rule (any public URL that passes the ADR 0019 fetch rules) |
| G16 | each later MCP server (P5A.16 on) | Owner approves every further MCP server, its credentials and its egress destination, one gate per destination |

## Assumptions to confirm (the design is silent)

- **A1** A chat with a named model records role `chat.manual`.
- **A2** The decider's "short summary of recent turns" is the last 3 turns cut to a fixed length by code, no model call.
- **A3** The decider's tier answer is a score over three levels; score = answer / 2.
- **A4** Price bands for tier suggestion are named constants in `constants.rs`, added in P1.1b and used by P1.9 (`suggest_tier()`); the owner confirms them in the PR.
- **A5** The user ceiling is global in `chat_auto.user_ceiling`, with an optional per-user override pushed by the admin.
- **A6** The server holds a conversation's current tier and sends `tier` plus `tier_user_set`; the gateway is stateless about conversations.
- **A7** An `unsafe` answer at or above `unsafe_block` refuses the chat with a new `ErrorCode` (`request_blocked`, 403) and a plain message.
- **A8** In-tier fallback applies to routed calls (Auto, role, tier); a named model keeps today's same-provider retries.
- **A9** Budget alerts become 75% and 90%; the 100% "budget reached" alert stays.
- **A10** Each provider's System One request and response shapes come from its reference, captured as fixtures in P2.3 and P2.3b.
- **A11** The design and this plan live in `docs/` as `design-tiered-routing.md` and `plan-tiered-routing.md`, with one file per phase in `docs/plan-tiered-routing/`.
- **A12** The MCP gateway's component name is `mcp-gateway` unless the MCP gateway ADR picks a less confusable one (ADR 0021 rule 4); its API prefix is chosen in the MCP gateway ADR and every step uses the ADR's names.
- **A13** An invalid plan is sent back to the planner once with the validation error codes (never tool output); a second invalid plan stops the task with a plain message. A `schema` check applies to the step's final JSON output.
