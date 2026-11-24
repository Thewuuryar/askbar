# Risks and open questions for the owner

Part of [the tiered routing plan](../plan-tiered-routing.md). For the owner; the orchestrator does not need this file.

- **Decider outage or latency.** Every Auto chat depends on it; failures go to `default_tier`; measure p95 per provider in shadow.
- **Hosted decider before the egress proxy.** The gateway reaches the internet directly until Phase 4. A hosted decider would run in shadow over that route, with the zero-retention record in place (G4). Alternative: a self-hosted decider only until Phase 4.
- **Self-hosted decider speed and licence.** OpenJev and Clef (both 27B) take seconds on a Mac mini M4 (OpenJev measured at 3 to 20 s); Clef-flash (9B) should take roughly a third of that (estimate, to be measured), still too slow for an Auto chat's latency budget unless measured fast enough, so a Mac fits shadow, judge pre-checks and local-only conversations. OpenJev's CC BY-NC weights need a commercial licence for this use; Clef and Clef-flash weights: check licence.
- **Decider maturity.** Jev is early access with no published p95 and overconfident probabilities; one benchmark reports Clef's calibration error at 0.02 to 0.04 on four of five datasets. Thresholds come only from Askbar's labeled sets, per decider, and a version change re-runs the check.
- **Undocumented usage addresses.** The two subscription usage endpoints may change; failures show "unknown" and never block calls.
- **Own MCP front is our code to maintain.** No third-party gateway (G13), so there is no upstream churn, but federation, stdio and OAuth to remote servers are not there: they come, with `rmcp`, only when G16 approves a third-party server. If the list outgrows a small Rust client, agentgateway is the runner-up as one upstream behind the front, after its own spike.
- **MCP tool servers are a new attack surface.** Each server holds credentials and returns untrusted output. Defences: signed approvals, per-user permissions off by default, audit before the call, fetch rules, egress default deny per destination, an owner gate per destination (G14 to G16).
- **Third-party image licence and CVE scan gap.** `searxng` is the one pulled image: AGPL-3.0, run unmodified as a separate container with a source link in the notices (the author's reading, not legal advice); the Trivy `images` job may not scan it (P5A.4); a CI change is the owner's.
- **Executor hardware.** Deferred with Phase 9; open only if the owner starts it (G10).
- **Training data.** 300 labels is the owner's work; shadow and the check report are not meaningful before it. Who labels, and when?
- **Dollar budgets.** The owner must enter dollar amounts and per-model prices before switching the unit; token budgets stay until then.
- **Assumptions A1 to A13** in [the plan](../plan-tiered-routing.md#assumptions-to-confirm-the-design-is-silent) need a yes or a change before the steps that use them.
