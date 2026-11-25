# Task tracker

As of 2026-11-24, on `main` at `0ad71c2`. One line for every numbered step in the plans listed in [plan-priorities.md](plan-priorities.md). The plans hold the step text; this file holds only the state.

A step is `[x]` only when its box is ticked on `main`. A step whose branch or pull request is open is In progress and stays `[ ]`. Tick the step's line here in the same commit that ticks its box in the plan. The admin redesign plan has no per-step boxes (finished, merged in #117), so it is not listed. The `W` stages are those of track 1 in [plan-priorities.md](plan-priorities.md#stages).

| Plan | Completed | In progress | Not started | Total |
| --- | --- | --- | --- | --- |
| [Refactor](plan-refactor.md) | 37 | 0 | 0 | 37 |
| [Order of work and web search](plan-priorities.md) | 3 | 0 | 8 | 11 |
| [Audit viewer](plan-audit-viewer.md) | 4 | 0 | 5 | 9 |
| [Tiered routing Phase 0: records and docs](plan-tiered-routing/phase-0-docs.md) | 2 | 0 | 15 | 17 |
| [Tiered routing Phase 1: records](plan-tiered-routing/phase-1-records.md) | 0 | 0 | 29 | 29 |
| [Tiered routing Phase 2: shadow](plan-tiered-routing/phase-2-shadow.md) | 0 | 0 | 20 | 20 |
| [Tiered routing Phase 3: Auto](plan-tiered-routing/phase-3-auto.md) | 0 | 0 | 28 | 28 |
| [Tiered routing Phase 4: egress proxy](plan-tiered-routing/phase-4-egress-proxy.md) | 9 | 1 | 13 | 23 |
| [Tiered routing Phase 5A: MCP gateway](plan-tiered-routing/phase-5a-mcp-gateway.md) | 4 | 0 | 37 | 41 |
| [Tiered routing Phase 5B: research and judge](plan-tiered-routing/phase-5b-research-judge.md) | 0 | 0 | 10 | 10 |
| [Tiered routing Phase 6: agents](plan-tiered-routing/phase-6-agents.md) | 0 | 0 | 31 | 31 |
| [Tiered routing Phase 8: admin UI](plan-tiered-routing/phase-8-admin-ui.md) | 0 | 0 | 10 | 10 |
| [Tiered routing Phase 9: executor](plan-tiered-routing/phase-9-executor.md) | 0 | 0 | 6 | 6 |
| [Tiered routing Phase 10: script library](plan-tiered-routing/phase-10-script-library.md) | 0 | 0 | 10 | 10 |
| [PII detection and redaction](plan-pii-redaction.md) | 0 | 0 | 11 | 11 |
| [Context checks and canaries](plan-canaries.md) | 0 | 0 | 9 | 9 |
| **Total** | **59** | **1** | **242** | **302** |

## Completed (59)

### Refactor (track 0)

- [x] **[R1.1](plan-refactor.md)** remove the unused span recorder from askbar-audit
- [x] **[R1.2](plan-refactor.md)** pin the audit chain hash
- [x] **[R1.3](plan-refactor.md)** drop the cost hook from audit stats
- [x] **[R1.4](plan-refactor.md)** group audit stats in SQL
- [x] **[R1.5](plan-refactor.md)** move askbar-audit unit tests to their own file
- [x] **[R1.6](plan-refactor.md)** one sqlite open and migrate helper
- [x] **[R1.7](plan-refactor.md)** one function interrupts a generation
- [x] **[R1.8](plan-refactor.md)** map a generation outcome in one place
- [x] **[R1.9](plan-refactor.md)** remove unused cancel and catalog code
- [x] **[R1.10](plan-refactor.md)** remove stream_chat
- [x] **[R1.11](plan-refactor.md)** remove unused split_model_id
- [x] **[R2.1](plan-refactor.md)** define docker helpers once in check-isolation
- [x] **[R2.2](plan-refactor.md)** one http helper in check-isolation
- [x] **[R2.3](plan-refactor.md)** admin setup and cleanup helpers in check-isolation
- [x] **[R2.4](plan-refactor.md)** check each service from one profile table
- [x] **[R2.5](plan-refactor.md)** check declared networks and mounts from compose config
- [x] **[R3.1](plan-refactor.md)** move service boot helpers into askbar-service
- [x] **[R3.2](plan-refactor.md)** share link routing with the alerter
- [x] **[R3.3](plan-refactor.md)** one table of link names
- [x] **[R3.4](plan-refactor.md)** share the append step of the append handlers
- [x] **[R3.5](plan-refactor.md)** remove a re-export, a single-use trait and an unused parameter
- [x] **[R3.6](plan-refactor.md)** one runtime base stage in the Dockerfile
- [x] **[R4.1](plan-refactor.md)** one token reader and env default for the links
- [x] **[R4.2](plan-refactor.md)** one link client for the alerter and gateway links
- [x] **[R4.3](plan-refactor.md)** split admin_api.rs into modules
- [x] **[R4.4](plan-refactor.md)** share fake link servers
- [x] **[R4.5](plan-refactor.md)** settings section and number field macros
- [x] **[R4.6](plan-refactor.md)** share audit page helpers and inline errors
- [x] **[R4.7](plan-refactor.md)** remove the unused button helper and ETag cache
- [x] **[R5.1](plan-refactor.md)** one mock model launcher and serve helper
- [x] **[R5.2](plan-refactor.md)** share test setup
- [x] **[R5.3](plan-refactor.md)** one invoke wrapper and one refresh in the popup
- [x] **[R5.4](plan-refactor.md)** one turn starter and shared action buttons
- [x] **[R5.5](plan-refactor.md)** match 404 by status and share the cancel slot
- [x] **[R5.6](plan-refactor.md)** one HTTP stub helper for the Python fixtures
- [x] **[RL.1](plan-refactor.md)** share normalize and last_turns
- [x] **[RL.2](plan-refactor.md)** decode stream events in one place

### Order of work and web search (track 1)

- [x] **[O0.1](plan-priorities.md)** amend 0019: order of work after 1.0 · W0
- [x] **[O0.2](plan-priorities.md)** point the tiered routing plan at the new order · W0
- [x] **[O0.3](plan-priorities.md)** reorder the post-v1 plan · W0

### Audit viewer (tracks 1 and 5)

- [x] **[AV0.2](plan-audit-viewer.md)** amend 0010, 0051 and 0052: step spans and payload roles · W0
- [x] **[AV2.1](plan-audit-viewer.md)** add the step span kind · W1
- [x] **[AV2.2](plan-audit-viewer.md)** seal the new message as the prompt payload · W1
- [x] **[AV2.3](plan-audit-viewer.md)** seal provider_request on the attempt · W1

### Tiered routing Phase 0: records and docs (track 4)

- [x] **[P0.1](plan-tiered-routing/phase-0-docs.md)** add the tiered routing design
- [x] **[P0.2](plan-tiered-routing/phase-0-docs.md)** replace the tiered routing plan

### Tiered routing Phase 4: egress proxy (track 1)

- [x] **[P4.1](plan-tiered-routing/phase-4-egress-proxy.md)** confirm ADR coverage for the egress proxy · W2
- [x] **[P4.1a](plan-tiered-routing/phase-4-egress-proxy.md)** add the host and address policy · W2
- [x] **[P4.1b](plan-tiered-routing/phase-4-egress-proxy.md)** serve the proxy listeners and /api/egress-proxy/v1 · W2
- [x] **[P4.1c](plan-tiered-routing/phase-4-egress-proxy.md)** add the admin-egress-proxy link · W2
- [x] **[P4.2](plan-tiered-routing/phase-4-egress-proxy.md)** add the egress proxy service · W2
- [x] **[P4.2a](plan-tiered-routing/phase-4-egress-proxy.md)** check the egress proxy's isolation · W2
- [x] **[P4.4](plan-tiered-routing/phase-4-egress-proxy.md)** push per-source allowlists to the proxy · W2
- [x] **[P4.4a](plan-tiered-routing/phase-4-egress-proxy.md)** approve private egress destinations per source · W2
- [x] **[P4.4b](plan-tiered-routing/phase-4-egress-proxy.md)** show blocked outbound requests · W2

### Tiered routing Phase 5A: MCP gateway (track 1)

- [x] **[P5A.1](plan-tiered-routing/phase-5a-mcp-gateway.md)** evaluate agentgateway as the MCP gateway · W3
- [x] **[P5A.2](plan-tiered-routing/phase-5a-mcp-gateway.md)** propose the MCP gateway and its front · W3
- [x] **[P5A.3](plan-tiered-routing/phase-5a-mcp-gateway.md)** add the agentgateway image pinned by digest · W3
- [x] **[P5A.7](plan-tiered-routing/phase-5a-mcp-gateway.md)** generate agentgateway config from pushed config · W3

## In progress (1)

P4.3a is committed on branch `feat/egress-proxy-wiring` (draft PR #144). CI is green on `f8cc0db`, including `isolation`. It is ticked here when the PR merges to `main`.

### Tiered routing Phase 4: egress proxy (track 1)

- [ ] **[P4.3a](plan-tiered-routing/phase-4-egress-proxy.md)** send model-gateway traffic through the proxy · W2

## Not started (242)

### Order of work and web search (track 1)

- [ ] **[W4.1](plan-priorities.md)** add model roles and role pins · W4
- [ ] **[W4.2](plan-priorities.md)** pin a model for each role · W4
- [ ] **[W5.1](plan-priorities.md)** add quick search types, limits and constants · W5
- [ ] **[W5.2](plan-priorities.md)** approve quick search calls by fixed rules · W5
- [ ] **[W5.3](plan-priorities.md)** answer a message with quick search · W5
- [ ] **[W5.4](plan-priorities.md)** search toggle and sources · W5
- [ ] **[W5.5](plan-priorities.md)** quick search with the mock model and a fixture search engine · W5
- [ ] **[W5.6](plan-priorities.md)** quick search in the user and admin guides · W5

### Audit viewer (tracks 1 and 5)

- [ ] **[AV0.1](plan-audit-viewer.md)** amend 0011: open a conversation as one action
- [ ] **[AV1.1](plan-audit-viewer.md)** filter traces by conversation
- [ ] **[AV1.2](plan-audit-viewer.md)** conversation list and open conversation
- [ ] **[AV1.3](plan-audit-viewer.md)** conversation view
- [ ] **[AV2.4](plan-audit-viewer.md)** show steps and side-by-side payloads

### Tiered routing Phase 0: records and docs (track 4)

- [ ] **[P0.3](plan-tiered-routing/phase-0-docs.md)** propose the tiered routing ADR
- [ ] **[P0.4](plan-tiered-routing/phase-0-docs.md)** amend 0014: tiers, deciders, dollar budgets, fallback
- [ ] **[P0.5](plan-tiered-routing/phase-0-docs.md)** amend 0002: routing after 1.0 and latency budgets
- [ ] **[P0.6](plan-tiered-routing/phase-0-docs.md)** amend 0010: decision records and task spans
- [ ] **[P0.7](plan-tiered-routing/phase-0-docs.md)** amend 0011: shared questions and hosted decider
- [ ] **[P0.8](plan-tiered-routing/phase-0-docs.md)** amend 0012: notices for sharing and the decider
- [ ] **[P0.9](plan-tiered-routing/phase-0-docs.md)** amend 0016: routing and library risks
- [ ] **[P0.10](plan-tiered-routing/phase-0-docs.md)** amend 0019: order, judge role, writer and pins
- [ ] **[P0.11](plan-tiered-routing/phase-0-docs.md)** amend 0020: approvals, library scripts and the executor deferred
- [ ] **[P0.12](plan-tiered-routing/phase-0-docs.md)** amend 0009: task state in the space file
- [ ] **[P0.13](plan-tiered-routing/phase-0-docs.md)** amend 0022: the judge is a role, not a caller
- [ ] **[P0.14](plan-tiered-routing/phase-0-docs.md)** add tiered routing terms to the architecture
- [ ] **[P0.15](plan-tiered-routing/phase-0-docs.md)** describe Auto, tiers and /decide in the architecture
- [ ] **[P0.16](plan-tiered-routing/phase-0-docs.md)** replaced by O0.3 (reorder the post-v1 plan)
- [ ] **[P0.17](plan-tiered-routing/phase-0-docs.md)** update the claude.ai project doc (owner)

### Tiered routing Phase 1: records (track 4)

- [ ] **[P1.1](plan-tiered-routing/phase-1-records.md)** add routing config and chat request fields
- [ ] **[P1.1a](plan-tiered-routing/phase-1-records.md)** add shared bounds and defaults for routing values
- [ ] **[P1.1e](plan-tiered-routing/phase-1-records.md)** share the generation timeout maximum
- [ ] **[P1.1b](plan-tiered-routing/phase-1-records.md)** add routing values to constants.rs
- [ ] **[P1.1c](plan-tiered-routing/phase-1-records.md)** push constants in each service's config
- [ ] **[P1.1d](plan-tiered-routing/phase-1-records.md)** amend 0028: constants in pushed config
- [ ] **[P1.2](plan-tiered-routing/phase-1-records.md)** accept and validate routing config
- [ ] **[P1.3](plan-tiered-routing/phase-1-records.md)** add weights and in-tier choice
- [ ] **[P1.4](plan-tiered-routing/phase-1-records.md)** record a routing decision on each chat span
- [ ] **[P1.5](plan-tiered-routing/phase-1-records.md)** look up spans by decision id
- [ ] **[P1.6](plan-tiered-routing/phase-1-records.md)** add a routing decision lookup
- [ ] **[P1.7](plan-tiered-routing/phase-1-records.md)** relay the decision id to clients
- [ ] **[P1.8](plan-tiered-routing/phase-1-records.md)** move prices to each model
- [ ] **[P1.9](plan-tiered-routing/phase-1-records.md)** add tiers, effort levels and weight overrides to models
- [ ] **[P1.10](plan-tiered-routing/phase-1-records.md)** push routing config to the gateway
- [ ] **[P1.11](plan-tiered-routing/phase-1-records.md)** records-only end to end
- [ ] **[P1.12](plan-tiered-routing/phase-1-records.md)** return token counts per model
- [ ] **[P1.13](plan-tiered-routing/phase-1-records.md)** compute spend in dollars
- [ ] **[P1.14](plan-tiered-routing/phase-1-records.md)** set budgets in dollars and switch the unit
- [ ] **[P1.15](plan-tiered-routing/phase-1-records.md)** alert at 75 and 90 percent of a budget
- [ ] **[P1.16](plan-tiered-routing/phase-1-records.md)** read subscription usage through cpa
- [ ] **[P1.17](plan-tiered-routing/phase-1-records.md)** poll subscription limits and estimate tokens left
- [ ] **[P1.18](plan-tiered-routing/phase-1-records.md)** alert at 75 and 90 percent of a subscription window
- [ ] **[P1.19](plan-tiered-routing/phase-1-records.md)** add a subscription limits route
- [ ] **[P1.20](plan-tiered-routing/phase-1-records.md)** amend 0054: routes, prices and decision records
- [ ] **[P1.21](plan-tiered-routing/phase-1-records.md)** amend 0051: decision fields and lookup
- [ ] **[P1.22](plan-tiered-routing/phase-1-records.md)** amend 0042: usage reads through api-call
- [ ] **[P1.23](plan-tiered-routing/phase-1-records.md)** amend 0039: relay the decision id
- [ ] **[P1.24](plan-tiered-routing/phase-1-records.md)** amend 0040: model prices, tiers, dollar budgets, subscription reads, constants.rs

### Tiered routing Phase 2: shadow (track 4)

- [ ] **[P2.1](plan-tiered-routing/phase-2-shadow.md)** add decider config
- [ ] **[P2.2](plan-tiered-routing/phase-2-shadow.md)** map scores to tiers
- [ ] **[P2.3](plan-tiered-routing/phase-2-shadow.md)** add a System One decision client
- [ ] **[P2.3b](plan-tiered-routing/phase-2-shadow.md)** add a second decider adapter
- [ ] **[P2.4](plan-tiered-routing/phase-2-shadow.md)** add a mock decider
- [ ] **[P2.5](plan-tiered-routing/phase-2-shadow.md)** build the chat questions and the state the decider sees
- [ ] **[P2.6](plan-tiered-routing/phase-2-shadow.md)** ask the decider in shadow beside the chosen model
- [ ] **[P2.7](plan-tiered-routing/phase-2-shadow.md)** add decider settings and the zero-retention record
- [ ] **[P2.8](plan-tiered-routing/phase-2-shadow.md)** store labeled examples
- [ ] **[P2.9](plan-tiered-routing/phase-2-shadow.md)** push labeled examples to the gateway
- [ ] **[P2.12](plan-tiered-routing/phase-2-shadow.md)** report accuracy on held-back examples
- [ ] **[P2.13](plan-tiered-routing/phase-2-shadow.md)** add a dry-run route
- [ ] **[P2.14](plan-tiered-routing/phase-2-shadow.md)** expose check and dry-run
- [ ] **[P2.15](plan-tiered-routing/phase-2-shadow.md)** report shadow results from decision records
- [ ] **[P2.17](plan-tiered-routing/phase-2-shadow.md)** amend 0006: private route to the self-hosted decider host
- [ ] **[P2.19](plan-tiered-routing/phase-2-shadow.md)** name the hosted decider's provider in the privacy notice
- [ ] **[P2.20](plan-tiered-routing/phase-2-shadow.md)** amend 0028: the decider key is pushed like provider keys
- [ ] **[P2.22](plan-tiered-routing/phase-2-shadow.md)** amend 0013: labeled examples are in admin.db
- [ ] **[P2.23](plan-tiered-routing/phase-2-shadow.md)** amend 0054: decider client, check and dry-run
- [ ] **[P2.24](plan-tiered-routing/phase-2-shadow.md)** amend 0040: decider settings and examples

### Tiered routing Phase 3: Auto (track 4)

- [ ] **[P3.1](plan-tiered-routing/phase-3-auto.md)** skip a failing model for five minutes
- [ ] **[P3.2](plan-tiered-routing/phase-3-auto.md)** fall back within the tier before the first token
- [ ] **[P3.3](plan-tiered-routing/phase-3-auto.md)** map effort to each provider's field
- [ ] **[P3.4](plan-tiered-routing/phase-3-auto.md)** route Auto chats
- [ ] **[P3.5](plan-tiered-routing/phase-3-auto.md)** step Auto down at the chat budget point
- [ ] **[P3.6](plan-tiered-routing/phase-3-auto.md)** run the decider beside the gates
- [ ] **[P3.7](plan-tiered-routing/phase-3-auto.md)** queue shared questions in memory
- [ ] **[P3.8](plan-tiered-routing/phase-3-auto.md)** store a conversation's tier and effort
- [ ] **[P3.9](plan-tiered-routing/phase-3-auto.md)** send roles and tiers to the gateway
- [ ] **[P3.10](plan-tiered-routing/phase-3-auto.md)** set a conversation's tier and effort
- [ ] **[P3.11](plan-tiered-routing/phase-3-auto.md)** ask again on a stronger model
- [ ] **[P3.12](plan-tiered-routing/phase-3-auto.md)** collect shared questions
- [ ] **[P3.13](plan-tiered-routing/phase-3-auto.md)** set Auto routing options
- [ ] **[P3.14](plan-tiered-routing/phase-3-auto.md)** lower accounts near their subscription limit
- [ ] **[P3.15](plan-tiered-routing/phase-3-auto.md)** list Auto in the catalog when live
- [ ] **[P3.16](plan-tiered-routing/phase-3-auto.md)** default new chats to Auto when listed
- [ ] **[P3.17](plan-tiered-routing/phase-3-auto.md)** show Auto and a reply's tier
- [ ] **[P3.18](plan-tiered-routing/phase-3-auto.md)** add tier and effort controls
- [ ] **[P3.19](plan-tiered-routing/phase-3-auto.md)** ask again and the shared marker
- [ ] **[P3.20](plan-tiered-routing/phase-3-auto.md)** Auto end to end
- [ ] **[P3.21](plan-tiered-routing/phase-3-auto.md)** measure decider latency against the 200 ms target
- [ ] **[P3.22](plan-tiered-routing/phase-3-auto.md)** describe Auto in the guide and the wire contract
- [ ] **[P3.23](plan-tiered-routing/phase-3-auto.md)** amend 0054: routing, fallback, effort, share queue
- [ ] **[P3.24](plan-tiered-routing/phase-3-auto.md)** amend 0013: shared questions in admin.db
- [ ] **[P3.25](plan-tiered-routing/phase-3-auto.md)** amend 0039: roles, tiers and ask again
- [ ] **[P3.26](plan-tiered-routing/phase-3-auto.md)** amend 0040: routing settings, shared questions, account demotion
- [ ] **[P3.27](plan-tiered-routing/phase-3-auto.md)** amend 0044: Auto, tier and effort controls
- [ ] **[P3.28](plan-tiered-routing/phase-3-auto.md)** amend 0023: request_blocked problem type

### Tiered routing Phase 4: egress proxy (track 1)

- [x] **[P4.3b](plan-tiered-routing/phase-4-egress-proxy.md)** send cpa traffic through the proxy · W2
- [x] **[P4.3c](plan-tiered-routing/phase-4-egress-proxy.md)** send alerter traffic through the proxy · W2
- [x] **[P4.3](plan-tiered-routing/phase-4-egress-proxy.md)** send gateway, cpa, alerter and searxng traffic through the proxy · W2
- [x] **[P4.5](plan-tiered-routing/phase-4-egress-proxy.md)** prove default deny and private-address blocking · W2
- [x] **[P4.6](plan-tiered-routing/phase-4-egress-proxy.md)** amend 0006: egress through the proxy · W2
- [x] **[P4.7](plan-tiered-routing/phase-4-egress-proxy.md)** amend 0032: proxy container limits · W2
- [x] **[P4.7a](plan-tiered-routing/phase-4-egress-proxy.md)** amend 0022: the egress proxy API prefix · W2
- [x] **[P4.7b](plan-tiered-routing/phase-4-egress-proxy.md)** amend 0049: the admin-egress-proxy link · W2
- [x] **[P4.7c](plan-tiered-routing/phase-4-egress-proxy.md)** amend 0054: the gateway reaches providers through the proxy · W2
- [x] **[P4.7d](plan-tiered-routing/phase-4-egress-proxy.md)** amend 0055: the alerter delivers through the proxy · W2
- [x] **[P4.7e](plan-tiered-routing/phase-4-egress-proxy.md)** amend 0042: cpa reaches providers through the proxy · W2
- [x] **[P4.7f](plan-tiered-routing/phase-4-egress-proxy.md)** amend 0040: the admin pushes the egress allowlists · W2
- [x] **[P4.8](plan-tiered-routing/phase-4-egress-proxy.md)** update the architecture network table and diagram · W2

### Tiered routing Phase 5A: MCP gateway (track 1)

- [x] **[P5.2](plan-tiered-routing/phase-5a-mcp-gateway.md)** add task, approval and decide types · W3
- [x] **[P5.5](plan-tiered-routing/phase-5a-mcp-gateway.md)** add the approval feature: keys, sign and verify · W3
- [x] **[P5A.31](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0036: the approval feature · W3
- [x] **[P5.3](plan-tiered-routing/phase-5a-mcp-gateway.md)** create and push the approval signing key · W3
- [x] **[P5.4](plan-tiered-routing/phase-5a-mcp-gateway.md)** sign approvals · W3
- [ ] **[P5A.4](plan-tiered-routing/phase-5a-mcp-gateway.md)** scan the searxng image for vulnerabilities · W3
- [ ] **[P5A.5](plan-tiered-routing/phase-5a-mcp-gateway.md)** add MCP gateway values to constants.rs · W3
- [ ] **[P5A.6](plan-tiered-routing/phase-5a-mcp-gateway.md)** add the front service with status and config push · W3
- [ ] **[P5A.6a](plan-tiered-routing/phase-5a-mcp-gateway.md)** add the server and admin links of the MCP gateway · W3
- [ ] **[P5A.8](plan-tiered-routing/phase-5a-mcp-gateway.md)** verify the signed approval before any tool call · W3
- [ ] **[P5A.9a](plan-tiered-routing/phase-5a-mcp-gateway.md)** add the mcp-gateway audit link · W3
- [ ] **[P5A.9](plan-tiered-routing/phase-5a-mcp-gateway.md)** write the audit span before each call · W3
- [ ] **[P5A.10](plan-tiered-routing/phase-5a-mcp-gateway.md)** check per-user tool permissions · W3
- [ ] **[P5A.11](plan-tiered-routing/phase-5a-mcp-gateway.md)** list tools and run approved calls through the registry · W3
- [ ] **[P5A.12](plan-tiered-routing/phase-5a-mcp-gateway.md)** add the mcp-gateway service and the fetch egress source · W3
- [ ] **[P5A.13](plan-tiered-routing/phase-5a-mcp-gateway.md)** add searxng · W3
- [ ] **[P5A.14](plan-tiered-routing/phase-5a-mcp-gateway.md)** add the native search tool · W3
- [ ] **[P5A.15](plan-tiered-routing/phase-5a-mcp-gateway.md)** add the native fetch tool and the ADR 0019 fetch rules · W3
- [ ] **[P5A.16](plan-tiered-routing/phase-5a-mcp-gateway.md)** register MCP servers and their credentials · W3
- [ ] **[P5A.17](plan-tiered-routing/phase-5a-mcp-gateway.md)** set per-user MCP tool permissions · W3
- [ ] **[P5A.18](plan-tiered-routing/phase-5a-mcp-gateway.md)** push MCP config to the front · W3
- [ ] **[P5A.19](plan-tiered-routing/phase-5a-mcp-gateway.md)** push the searxng and fetch egress entries · W3
- [ ] **[P5A.20](plan-tiered-routing/phase-5a-mcp-gateway.md)** refuse unsigned, replayed, swapped and expired approvals · W3
- [ ] **[P5A.21](plan-tiered-routing/phase-5a-mcp-gateway.md)** refuse the call when the audit write fails or the permission is off · W3
- [ ] **[P5A.22](plan-tiered-routing/phase-5a-mcp-gateway.md)** the fetch SSRF matrix at the front · W3
- [ ] **[P5A.23](plan-tiered-routing/phase-5a-mcp-gateway.md)** search and fetch end to end · W3
- [ ] **[P5A.24](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0019: the MCP gateway is an own front with native tools · W3
- [ ] **[P5A.25](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0028: MCP gateway config and the approval public key · W3
- [ ] **[P5A.26](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0032: MCP gateway and searxng container limits · W3
- [ ] **[P5A.27](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0017: the one third-party image is searxng · W3
- [ ] **[P5A.28](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0026: scan third-party images · W3
- [ ] **[P5A.29](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0016: tool servers as an attack surface · W3
- [ ] **[P5A.30](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0056: an any_public entry for fetch · W3
- [ ] **[P5A.32](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0022: the MCP gateway API prefix · W3
- [ ] **[P5A.33](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0049: the MCP gateway links · W3
- [ ] **[P5A.34](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0010: the mcp-gateway audit link · W3
- [ ] **[P5A.35](plan-tiered-routing/phase-5a-mcp-gateway.md)** amend 0051: the mcp-gateway may start tool spans · W3

### Tiered routing Phase 5B: research and judge (tracks 1 and 4)

- [ ] **[P5.6](plan-tiered-routing/phase-5b-research-judge.md)** add POST /decide for the judge role · W7
- [ ] **[P5.7](plan-tiered-routing/phase-5b-research-judge.md)** run the decider cascade for the judge in shadow
- [ ] **[P5.8](plan-tiered-routing/phase-5b-research-judge.md)** add step_check and research decisions to /decide
- [ ] **[P5.9](plan-tiered-routing/phase-5b-research-judge.md)** add task tables and mark running tasks interrupted on start · W7
- [ ] **[P5.10](plan-tiered-routing/phase-5b-research-judge.md)** enforce the tool-call limit and task cost cap · W7
- [ ] **[P5.13](plan-tiered-routing/phase-5b-research-judge.md)** run research reports · W7
- [ ] **[P5.14](plan-tiered-routing/phase-5b-research-judge.md)** add role pins and the role table
- [ ] **[P5.15](plan-tiered-routing/phase-5b-research-judge.md)** set roles, decider thresholds and task caps
- [ ] **[P5.17](plan-tiered-routing/phase-5b-research-judge.md)** amend 0004 and 0049, 0023 · W7
- [ ] **[P5.18](plan-tiered-routing/phase-5b-research-judge.md)** amend 0054, 0039, 0040 for research and the judge · W7

### Tiered routing Phase 6: agents (track 4)

- [ ] **[P6.1](plan-tiered-routing/phase-6-agents.md)** add plan and step types
- [ ] **[P6.2](plan-tiered-routing/phase-6-agents.md)** add agent values to constants.rs and push them to the server
- [ ] **[P6.3](plan-tiered-routing/phase-6-agents.md)** read the user's permitted MCP tools through the front
- [ ] **[P6.4](plan-tiered-routing/phase-6-agents.md)** ask the planner for a plan limited to the permitted tools
- [ ] **[P6.5](plan-tiered-routing/phase-6-agents.md)** validate the plan before any step runs
- [ ] **[P6.6](plan-tiered-routing/phase-6-agents.md)** run a worker step with its step context only
- [ ] **[P6.7](plan-tiered-routing/phase-6-agents.md)** send each tool call through the judge, a signed approval and the MCP front
- [ ] **[P6.8](plan-tiered-routing/phase-6-agents.md)** check steps against a JSON schema
- [ ] **[P6.9](plan-tiered-routing/phase-6-agents.md)** review a step with the planner when no deterministic check applies
- [ ] **[P6.10](plan-tiered-routing/phase-6-agents.md)** check steps with the decider's pass answer
- [ ] **[P6.11](plan-tiered-routing/phase-6-agents.md)** retry once on standard, then the fixer
- [ ] **[P6.12](plan-tiered-routing/phase-6-agents.md)** stop at the tool-call limit or cost cap and answer from finished work
- [ ] **[P6.13](plan-tiered-routing/phase-6-agents.md)** mark provider failures interrupted and resume
- [ ] **[P6.14](plan-tiered-routing/phase-6-agents.md)** add the client API to start an agent task and stream progress
- [ ] **[P6.15](plan-tiered-routing/phase-6-agents.md)** add the client API to resume and cancel a task
- [ ] **[P6.16](plan-tiered-routing/phase-6-agents.md)** add planner, worker and fixer role defaults
- [ ] **[P6.17](plan-tiered-routing/phase-6-agents.md)** set pins for the planner, worker and fixer
- [ ] **[P6.18](plan-tiered-routing/phase-6-agents.md)** shadow decider step checks against the planner
- [ ] **[P6.19](plan-tiered-routing/phase-6-agents.md)** report step-check shadow results
- [ ] **[P6.20](plan-tiered-routing/phase-6-agents.md)** show agent tasks and progress
- [ ] **[P6.21](plan-tiered-routing/phase-6-agents.md)** add Resume, Cancel and the rerun confirmation
- [ ] **[P6.22](plan-tiered-routing/phase-6-agents.md)** add a mock MCP tool server
- [ ] **[P6.23](plan-tiered-routing/phase-6-agents.md)** script mock LLM plans
- [ ] **[P6.24](plan-tiered-routing/phase-6-agents.md)** run agent tasks against the mock tool server and scripted plans
- [ ] **[P6.25](plan-tiered-routing/phase-6-agents.md)** tool output that asks for other tools or settings is refused
- [ ] **[P6.26](plan-tiered-routing/phase-6-agents.md)** agent task smoke
- [ ] **[P6.27](plan-tiered-routing/phase-6-agents.md)** amend 0019: agents use MCP tools, executor deferred
- [ ] **[P6.28](plan-tiered-routing/phase-6-agents.md)** amend 0054: agent roles and step checks
- [ ] **[P6.29](plan-tiered-routing/phase-6-agents.md)** amend 0039: the agent loop and task API
- [ ] **[P6.30](plan-tiered-routing/phase-6-agents.md)** amend 0040: agent role pins and step-check report
- [ ] **[P6.31](plan-tiered-routing/phase-6-agents.md)** amend 0044: agent tasks, Resume and rerun confirmation

### Tiered routing Phase 8: admin UI (tracks 1 and 4)

- [ ] **[P8.2](plan-tiered-routing/phase-8-admin-ui.md)** add the routing page
- [ ] **[P8.3](plan-tiered-routing/phase-8-admin-ui.md)** add decider settings
- [ ] **[P8.4](plan-tiered-routing/phase-8-admin-ui.md)** add the dry-run panel and decider comparison
- [ ] **[P8.5](plan-tiered-routing/phase-8-admin-ui.md)** add example labeling and shared-question review
- [ ] **[P8.6](plan-tiered-routing/phase-8-admin-ui.md)** add spend and subscription limit views
- [ ] **[P8.6a](plan-tiered-routing/phase-8-admin-ui.md)** add MCP server and tool permission pages · W6
- [ ] **[P8.6b](plan-tiered-routing/phase-8-admin-ui.md)** add agent task views
- [ ] **[P8.7](plan-tiered-routing/phase-8-admin-ui.md)** add script review
- [ ] **[P8.8](plan-tiered-routing/phase-8-admin-ui.md)** cover the routing, MCP and task pages at 1280 and 390
- [ ] **[P8.9](plan-tiered-routing/phase-8-admin-ui.md)** update the admin guide

### Tiered routing Phase 9: executor (deferrable)

- [ ] **[P9.1](plan-tiered-routing/phase-9-executor.md)** add executor work and output types
- [ ] **[P9.2](plan-tiered-routing/phase-9-executor.md)** add the service, pairing and approval check
- [ ] **[P9.3](plan-tiered-routing/phase-9-executor.md)** add executor-link
- [ ] **[P9.4](plan-tiered-routing/phase-9-executor.md)** add code and file tools that run in the executor
- [ ] **[P9.5](plan-tiered-routing/phase-9-executor.md)** check steps by exit code and tests
- [ ] **[P9.6](plan-tiered-routing/phase-9-executor.md)** amend 0020, 0054, 0039, 0040 for the executor

### Tiered routing Phase 10: script library (deferrable)

- [ ] **[P10.1](plan-tiered-routing/phase-10-script-library.md)** add the script catalog and entry format
- [ ] **[P10.2](plan-tiered-routing/phase-10-script-library.md)** run tests for a draft in a sandbox
- [ ] **[P10.3](plan-tiered-routing/phase-10-script-library.md)** author drafts from finished tasks
- [ ] **[P10.4](plan-tiered-routing/phase-10-script-library.md)** review, promote, edit and reject scripts (API)
- [ ] **[P10.5](plan-tiered-routing/phase-10-script-library.md)** match requests to scripts in shadow
- [ ] **[P10.6](plan-tiered-routing/phase-10-script-library.md)** fill inputs and validate against the schema
- [ ] **[P10.7](plan-tiered-routing/phase-10-script-library.md)** run a matched script with a signed approval
- [ ] **[P10.8](plan-tiered-routing/phase-10-script-library.md)** track success, demote and retire scripts
- [ ] **[P10.9](plan-tiered-routing/phase-10-script-library.md)** unpromoted or changed scripts do not run
- [ ] **[P10.10](plan-tiered-routing/phase-10-script-library.md)** amend 0016, 0054, 0039, 0040 for the library

### PII detection and redaction (track 2)

- [ ] **[PII-P0.1](plan-pii-redaction.md)** amend 0054 and 0014: PII detection and placeholders
- [ ] **[PII-P0.2](plan-pii-redaction.md)** amend 0011 and 0051: provider request payload and PII attributes
- [ ] **[PII-P1.1](plan-pii-redaction.md)** add PII settings
- [ ] **[PII-P1.2](plan-pii-redaction.md)** detect PII and record counts
- [ ] **[PII-P2.1](plan-pii-redaction.md)** replace PII with placeholders
- [ ] **[PII-P2.2](plan-pii-redaction.md)** add the reply filter and restore placeholders
- [ ] **[PII-P2.3](plan-pii-redaction.md)** refuse a turn with blocked PII
- [ ] **[PII-P2.4](plan-pii-redaction.md)** record the redact step
- [ ] **[PII-P2.6](plan-pii-redaction.md)** check search queries and fetch URLs for PII
- [ ] **[PII-P3.1](plan-pii-redaction.md)** store and push PII settings
- [ ] **[PII-P3.2](plan-pii-redaction.md)** Privacy page

### Context checks and canaries (track 3)

- [ ] **[C0.1](plan-canaries.md)** amend 0054 and 0014: context checks and canaries
- [ ] **[C0.2](plan-canaries.md)** amend 0051 and 0055: canary attributes and ContextTruncated
- [ ] **[C1.1](plan-canaries.md)** record the provider's finish reason
- [ ] **[C1.2](plan-canaries.md)** compare reported input tokens with the estimate
- [ ] **[C2.1](plan-canaries.md)** add a context limit to providers
- [ ] **[C2.2](plan-canaries.md)** trim history to the provider's context limit
- [ ] **[C3.1](plan-canaries.md)** plant and check the context canary
- [ ] **[C3.2](plan-canaries.md)** alert on truncated context
- [ ] **[C3.3](plan-canaries.md)** context canary checkbox
