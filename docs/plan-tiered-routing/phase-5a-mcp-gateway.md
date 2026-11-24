# Phase 5A: MCP gateway (branch `feat/mcp-gateway`)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

The MCP gateway is agentgateway (Linux Foundation AAIF project, Rust, Apache-2.0) behind a thin Rust front service, following the `cpa` pattern (a third-party image wrapped by a Rust supervisor, ADR 0037; see `services/cpa-supervisor`). agentgateway does MCP federation, transports, tool servers and their credentials, and per-user per-tool CEL rules; its `mcpAuthorization` CEL cannot read tool arguments or call out. The front does what it can't: verify the signed approval (Ed25519 over the exact call hash, one-time ID, 60 s expiry), write the audit span before the call through the Rust `AuditClient` (`crates/askbar-service`; fail closed), check per-user tool permissions, enforce ADR 0019's fetch rules, serve `GET /status` with the config version and take config pushes (ADR 0028). agentgateway listens only inside the container or private network, so nothing reaches it except through the front. Fallback: IBM ContextForge (Python, Apache-2.0; needs PostgreSQL and Redis; in-process plugins). Rejected: Docker MCP Gateway (spawns containers, conflicts with no Docker socket), MetaMCP (no per-user permissions or audit found), Microsoft MCP Gateway (Kubernetes, per-adapter access).

Needs Phase 4, G0, G9. Several agentgateway facts are unverified (stdio servers as subprocesses without Docker, HTTP proxy support for the egress proxy, JWT algorithms, memory footprint, release cadence), so P5A.1 is a spike that ends at G13. If the owner picks ContextForge, stop: rewrite P5A.3, P5A.4, P5A.7 and P5A.12 (PostgreSQL and Redis containers, plugins in place of parts of the front) and commit that rewrite before going on. No ADR defines the MCP gateway yet (ADR 0001 rule 3: a new image or service needs one), so nothing past P5A.2 starts until the owner accepts the MCP gateway ADR (G9). Values introduced here (approval expiry 60 s, fetch size and time limits, the one-time ID window) go in `constants.rs`, with bounds in `askbar-proto` where a service checks them; the front gets them in config pushes (ADR 0028). Tool output is untrusted data everywhere. The MCP gateway holds only tool credentials, has no route to `/api/server/v1` or the admin API, and its egress is default deny per destination (ADR 0019). `search` and `fetch` are each a per-user permission, off by default.

- [ ] **P5A.1** `docs(docs): evaluate agentgateway as the MCP gateway` · sonnet · after Phase 4
  Files: `docs/mcp-gateway-evaluation.md`; a scratch compose file outside the repo (not committed; its content is quoted in the doc).
  Do: in a scratch compose, check each unverified item and record a yes, no or unknown with the command and output that show it: (1) a stdio MCP server runs as a subprocess without Docker or a Docker socket; (2) agentgateway reaches a tool server and an upstream through an HTTP proxy (the egress proxy from Phase 4); (3) which JWT algorithms it accepts for the user identity the front passes; (4) memory idle and under a short load; (5) release cadence, versions, image digest and architectures, licence file. Also confirm `mcpAuthorization` per-user per-tool CEL works and cannot read arguments. For the fallback, record ContextForge's footprint (PostgreSQL, Redis) and plugin hook points. Pin nothing yet.
  Accept: every item answered; the doc ends with a recommendation and the changes to the plan if ContextForge is picked; `scripts/check-docs.sh` passes. Gate: owner picks agentgateway or ContextForge (G13).
- [ ] **P5A.2** `docs(adr): propose the MCP gateway and its front` · sonnet · after P5A.1, G13
  Files: `docs/adr/NNNN-mcp-gateway.md` (from `0000-template.md`), `docs/adr/README.md` index row.
  Do: Status Proposed. The component name (A12: not confusable, ADR 0021 rule 4, ADR 0038), the API audience's path prefix and OpenAPI file (`docs/openapi/mcp-gateway.v1.yaml`, ADR 0022), the split between the front and the chosen software, networks (the software on an internal network only, the front the one door), hardening per ADR 0032 (`read_only`, `cap_drop: ALL`, `no-new-privileges`, `pids_limit`, memory limit, tmpfs `/tmp`), image pinned by digest with licence notices (ADR 0017), holds only tool credentials, no route to `/api/server/v1` or the admin API, egress default deny per destination, untrusted tool output, the fetch rules, the fallback and the rejected options. Link the amendments in P5A.24 to P5A.29. Do not write "Accepted".
  Accept: ADR format check passes; README row added. Gate: owner accepts (G9).
- [ ] **P5.2** `feat(proto): add task, approval and decide types` · haiku · after P5A.2
  Files: `crates/askbar-proto/src/{task,approval,decide}.rs` (new), `lib.rs` (mod lines), tests.
  Do: `Task`, `TaskStatus` (`running`, `done`, `stopped`, `interrupted`), `Approval {action_hash, task_id, user, one_time_id, expires_at, signature}`, `ToolAction {tool, arguments, destination}`, `DecideRequest` and `DecideResponse` (use `judge`, `step_check`, `research`). Additive, `#[serde(default)]`.
  Accept: round-trip tests; old payloads without the fields parse; the hash of an action is stable across field order (canonical JSON).
- [ ] **P5.3** `feat(admin): create and push the approval signing key` · sonnet · after P5.2, G0, G9
  Files: `services/admin/src/{secrets,gateway_link}.rs`, tests, OpenAPI if a route is added.
  Do: Ed25519 key made by the admin, pushed to `model-gateway` and held in its memory only (like provider keys, ADR 0028); the public key is pushed to the MCP gateway front; rotation re-pushes both; never in logs, `/status`, audit attributes or any API response.
  Accept: tests: key absent from every output; rotation changes the public key version; a service before its first push signs and verifies nothing (fail closed).
- [ ] **P5.4** `feat(model-gateway): sign approvals` · sonnet · after P5.3
  Files: `services/model-gateway/src/{approval,lib}.rs` (new `approval.rs`), `tests/api.rs`.
  Do: sign the hash of the exact action, task, user, a one-time ID and a 60 s expiry (expiry from `constants.rs`, bounds in `askbar-proto`); record each approval in the audit span (hash and ID, no arguments). Signing is called by `/decide` in P5.6; here it is a function with its own tests.
  Accept: a changed argument changes the hash; expiry and one-time ID are present; no key in any output.
- [ ] **P5.5** `feat(proto): verify approvals` · sonnet · after P5.2
  Files: `crates/askbar-proto/src/approval.rs`, tests (or a new crate if the owner chose that at G9).
  Do: verify signature, action hash, expiry and one-time ID against the public key; the one-time ID store is a small trait the caller supplies; shared by the MCP gateway front and, in Phase 9, the executor. Every failure is a distinct error code.
  Accept: tests with a signed fixture: valid passes; unsigned, wrong key, swapped action, expired, replayed (second use) each fail with its own code.
- [ ] **P5A.3** `feat(compose): add the agentgateway image pinned by digest` · sonnet · after P5A.2, P5.5
  Files: `Dockerfile`, `compose.yaml` (build target only, not started yet), `scripts/third-party-notices.images.md`, `scripts/third-party-notices.sh`, `scripts/check-cpa-image.sh` pattern for a new `scripts/check-mcp-image.sh`.
  Do: a build stage that takes agentgateway by digest (ADR 0017), no tag-only reference; the licence text copied into the notices as for CLIProxyAPI; a check that the digest and version in the Dockerfile match the notices. New image: stop-and-ask until G9 is cleared.
  Accept: the check script passes; the notices name the version and licence (Apache-2.0); `cargo deny` untouched (no new crate).
- [ ] **P5A.4** `feat(compose): scan the agentgateway image for vulnerabilities` · sonnet · after P5A.3
  Files: `scripts/scan-third-party-images.sh` (new), `docs/mcp-gateway-evaluation.md` (a "scan" paragraph).
  Do: the Trivy `images` CI job (`.github/workflows/images.yml`) scans pushed digests with its flags and `.trivyignore.yaml`; first work out whether it covers a pulled third-party image such as agentgateway and `cpa`'s base (a gap if it does not). Add a local script that runs the same Trivy flags on every digest the Dockerfile pins from outside, and write the exact workflow change for the owner. The orchestrator never edits `.github/workflows/`.
  Accept: the script runs on the agentgateway image; the PR body states whether CI already covered it and lists the workflow change for the owner. Gate: the owner applies or declines the workflow change.
- [ ] **P5A.5** `feat(admin): add MCP gateway values to constants.rs` · haiku · after P5.2, G0
  Files: `crates/askbar-proto/src/mcp.rs` (new), `services/admin/src/constants.rs`, tests.
  Do: `MIN_`/`MAX_`/`DEFAULT_` for the fetch size limit, fetch time limit, tool-call time limit, the one-time ID window, and the maximum search URLs remembered per task; each with a doc comment; the approval expiry (60 s) is reused from P1.1a. Added to the struct served as `/admin/constants.json`.
  Accept: tests `MIN <= DEFAULT <= MAX`; the served JSON has no secret-looking keys; existing constants tests pass untouched.
- [ ] **P5A.6** `feat(mcp-gateway): add the front service with status and config push` · sonnet · after P5A.3, P5A.5
  Files: `services/mcp-gateway/{Cargo.toml,src/{main,lib,config,status}.rs,tests/api.rs}` (name per the MCP gateway ADR, A12), workspace `Cargo.toml`, `docs/openapi/mcp-gateway.v1.yaml`.
  Do: a Rust service built on `askbar-service`; a supervisor starts and restarts agentgateway as a child process (the `cpa-supervisor` pattern); `GET /status` shows the config version and the agentgateway state, no keys or arguments; `PUT /config` takes pushes and validates them against the `askbar-proto` bounds, keeping the old config on a bad push; before its first push it refuses every tool call (fail closed) and uses the `askbar-proto` defaults.
  Accept: tests: bad push leaves old config live; version hash changes with content; `/status` shows no secret; OpenAPI test passes.
- [ ] **P5A.7** `feat(mcp-gateway): generate agentgateway config from pushed config` · sonnet · after P5A.6
  Files: `services/mcp-gateway/src/agw_config.rs` (new), tests.
  Do: turn the pushed registry (servers, transports, credentials, per-user tool rules as CEL where agentgateway can express them) into the agentgateway config file in a tmpfs path; credentials only in that file and memory, never in logs, `/status` or errors; reload or restart agentgateway on a changed config and report the version it runs.
  Accept: golden-file tests for the generated config; a test greps every output for a marker credential and finds nothing; agentgateway listens on the internal address only (a test checks the generated listener address).
- [ ] **P5A.8** `feat(mcp-gateway): verify the signed approval before any tool call` · sonnet · after P5.5, P5A.6
  Files: `services/mcp-gateway/src/{approval,call}.rs`, tests.
  Do: `POST` call route takes the tool, exact arguments and the approval; recompute the action hash, run the P5.5 verify with the pushed public key and an in-memory one-time ID store (window from `constants.rs`); any failure refuses with `problem+json` (ADR 0023) and a distinct code, before anything reaches agentgateway.
  Accept: unsigned, replayed, swapped, expired and wrong-key approvals are each refused; a valid one passes to the next check.
- [ ] **P5A.9a** `feat(auditor): add the mcp-gateway audit link` · sonnet · after P5A.6
  Files: `services/auditor/src/lib.rs` (`LINKS`, `may_start`), `compose.yaml`, `services/bootstrap/src/{links,write}.rs`, `scripts/check-isolation.sh`, tests.
  Do: add an `mcp-gateway-auditor` entry to `LINKS` with its bootstrap token; add an audit network entry for the front in compose; add a `may_start` arm for `mcp-gateway` that allows `tool` spans only. Amend ADR 0010 and ADR 0051 in separate docs commits (the link and the arm).
  Accept: the front can open a tool span and nothing else (a `turn` or `gate` start is refused); `scripts/check-isolation.sh` passes.
- [ ] **P5A.9** `feat(mcp-gateway): write the audit span before each call` · sonnet · after P5A.8, P5A.9a
  Files: `services/mcp-gateway/src/{audit,call}.rs`, `docs/style/logging-and-audit.md`.
  Do: through the Rust `AuditClient` open the span (user, task, tool, approval ID, argument hash, no argument text) and call only after the auditor acknowledges; the full fetch URL is recorded in its own span before a fetch (P5A.15); if the auditor does not acknowledge, refuse the call (fail closed); close the span with the outcome.
  Accept: a test with an unreachable auditor refuses the call and agentgateway sees nothing; a log grep for a marker argument finds nothing.
- [ ] **P5A.10** `feat(mcp-gateway): check per-user tool permissions` · sonnet · after P5A.9
  Files: `services/mcp-gateway/src/permissions.rs` (new), tests.
  Do: pushed config holds, per user, the permitted tools (`search` and `fetch` off by default, ADR 0019); the front refuses a call to a tool the user does not hold, and `GET` tools lists only the user's permitted tools; the same rule also goes into agentgateway's CEL where it can, as a second barrier.
  Accept: permission off refuses with its own code; a user with no entry holds nothing; the list shows only permitted tools.
- [ ] **P5A.11** `feat(mcp-gateway): list tools and forward approved calls to agentgateway` · sonnet · after P5A.7, P5A.10
  Files: `services/mcp-gateway/src/{tools,call}.rs`, `docs/openapi/mcp-gateway.v1.yaml`, tests with a mock MCP server.
  Do: `GET` tools returns the user's permitted tools (name and argument schema); the call route forwards a call that passed the approval, audit and permission checks to agentgateway over the internal address, applies the tool-call time limit and a response size limit (both from `constants.rs`), and returns the result marked as untrusted tool output.
  Accept: tests with a mock tool server: list, call, timeout, oversize reply; a call that failed any earlier check never reaches the mock.
- [ ] **P5A.12** `feat(compose): add the mcp-gateway service` · sonnet · after P5A.11, P5A.4
  Files: `compose.yaml`, `Dockerfile`, `scripts/check-isolation.sh`.
  Do: the front and agentgateway in one container; hardening per ADR 0032 (`read_only`, `cap_drop: ALL`, `no-new-privileges`, `pids_limit`, memory limit, tmpfs `/tmp`); the network from the MCP gateway ADR: reachable by `server` only, no route to `/api/server/v1` or the admin API, outbound only through the egress proxy; no published port. New container, network: stop-and-ask until G9.
  Accept: `scripts/check-isolation.sh` proves the isolation rules and that nothing but the front answers on the agentgateway port.
- [ ] **P5A.13** `feat(compose): add searxng` · haiku · after P5A.12
  Files: `compose.yaml`, `scripts/check-isolation.sh`, `scripts/third-party-notices.images.md`.
  Do: SearXNG image pinned by digest, notices and scan as in P5A.3 and P5A.4; hardened per ADR 0032; reachable only from the MCP gateway; outbound only through the egress proxy.
  Accept: isolation check passes; no published port.
- [ ] **P5A.14** `feat(mcp-gateway): add the search MCP server` · sonnet · after P5A.13, P5A.10
  Files: `services/mcp-gateway/src/search.rs` or an off-the-shelf SearXNG MCP server image (chosen by the spike; a new image or dependency is a stop-and-ask), `agw_config.rs`, tests.
  Do: register a `search` tool with agentgateway that queries searxng; the front remembers the URLs of each task's results (bounded count from `constants.rs`) for the fetch rules; the tool is off for every user until the admin grants it.
  Accept: a mock searxng returns results; results are marked untrusted; a user without the permission is refused. Gate: owner approves the server and its destinations (G14).
- [ ] **P5A.15** `feat(mcp-gateway): add the fetch MCP server and the ADR 0019 fetch rules` · sonnet · after P5A.14, P5A.9
  Files: `services/mcp-gateway/src/{fetch,fetch_rules}.rs`, tests.
  Do: a `fetch` tool; the front enforces ADR 0019's rules (read the ADR text first): fetch only a URL that came from this task's search results or from the user (the server marks it, and the signed action covers the URL, so the judge saw it); size and time limits from `constants.rs`; resolve the host and refuse private, loopback and link-local addresses; redirects re-checked; the full URL goes in the audit span before the fetch runs; the fetched page is untrusted data.
  Accept: unit tests per rule (unlisted URL, oversize, slow, redirect to a private address, private address by name and by literal); the span exists before the request. Gate: owner approves the server and its egress rule (G15).
- [ ] **P5A.16** `feat(admin): register MCP servers and their credentials` · sonnet · after P5A.6, G0, G16
  Files: `services/admin/src/{mcp,secrets,db}.rs` (new `mcp.rs`), one route line each in `lib.rs`, `docs/openapi/admin.v1.yaml`, tests.
  Do: append a migration for MCP servers (name, transport, address or command, enabled); an API to create, list, update and delete them; each credential is stored as provider keys are, sent only to the front and never returned; values and limits in `constants.rs`; each change is an admin action. Search and fetch are registered as built-in entries.
  Accept: a credential never appears in any response, log or audit attribute; an unregistered server cannot be called; OpenAPI test passes. Gate: each new server is a separate owner approval (G16).
- [ ] **P5A.17** `feat(admin): set per-user MCP tool permissions` · sonnet · after P5A.16
  Files: `services/admin/src/mcp.rs`, `db.rs` migration, OpenAPI, tests.
  Do: per user and tool: allow or deny, default off for every tool (ADR 0019); an API to read and set them; each change is an admin action; deleted with the user.
  Accept: a user with no row holds nothing; set and read round-trip; user-delete test removes rows.
- [ ] **P5A.18** `feat(admin): push MCP config to the front` · sonnet · after P5A.17, P5A.7, P5.3
  Files: `services/admin/src/mcp_link.rs` (new), tests.
  Do: push registry, credentials, permissions, the constants it uses and the approval public key to the front's `PUT /config`, re-pushing on a version mismatch at the interval in `constants.rs` (ADR 0028); a rejected push shows its error and does not loop faster than the interval.
  Accept: a fake front receives the push; a changed permission changes the version; an old front that ignores new fields still accepts.
- [ ] **P5A.19** `feat(admin): push egress allowlist entries per MCP server destination` · sonnet · after P5A.18, P4.4
  Files: `services/admin/src/{mcp,egress_link}.rs`, tests.
  Do: for each enabled MCP server, one allowlist entry per destination, sourced to the `mcp-gateway`, through the mechanism P4.4 built; nothing outside the registry is allowed; a server registered without a destination has no egress. Each new destination is an owner gate (G14, G15, G16).
  Accept: tests: a registered destination appears in the push, a removed one disappears; a private address is refused unless the owner approved it as a host.
- [ ] **P5A.20** `test(mcp-gateway): refuse unsigned, replayed, swapped and expired approvals` · sonnet · after P5A.8
  Files: `services/mcp-gateway/tests/approvals.rs`.
  Do: ADR 0016 "Defend": a signed approval from the real signer path (P5.4) is replayed, its arguments swapped, its expiry passed, its signature stripped, and signed with another key.
  Accept: each is refused before any call reaches the mock tool server, and each is recorded.
- [ ] **P5A.21** `test(mcp-gateway): refuse the call when the audit write fails or the permission is off` · sonnet · after P5A.9, P5A.10
  Files: `services/mcp-gateway/tests/{audit,permissions}.rs`.
  Do: an auditor that is down, slow or answers an error; a permitted and a not-permitted user; a permission removed by a config push between two calls.
  Accept: the call is refused in each failing case and reaches the mock tool server in none; the allowed case passes.
- [ ] **P5A.22** `test(mcp-gateway): enforce the fetch rules and block private addresses` · sonnet · after P5A.15
  Files: `services/mcp-gateway/tests/fetch.rs`.
  Do: URLs not from search results or the user, oversize and slow responses, redirects, private, loopback and link-local addresses, a host that resolves to one, and a literal IP.
  Accept: each refused; the full URL appears in the audit span before every allowed fetch.
- [ ] **P5A.23** `test(mcp-gateway): search and fetch end to end` · haiku · after P5A.19, P5A.22
  Files: `scripts/smoke.sh`, `tests/e2e/` fixtures.
  Do: smoke step with a mock searxng and a mock page: sign an approval, search, fetch a result, check the audit spans and that an unsigned call and a direct request to the agentgateway port are refused.
  Accept: `scripts/smoke.sh` passes.
- [ ] **P5A.24** `docs(adr): amend 0019: the MCP gateway is agentgateway behind a front` · haiku · after P5A.15. Files: `0019-post-v1-order-and-tools.md`. The fetch rules are enforced in the front. Decision.
- [ ] **P5A.25** `docs(adr): amend 0028: MCP gateway config and the approval public key` · haiku · after P5A.18. Files: `0028-service-boundaries-and-config-delivery.md`. The front takes config pushes with constants, permissions and the approval public key; tool credentials are pushed and held in memory. Decision.
- [ ] **P5A.26** `docs(adr): amend 0032: MCP gateway and searxng container limits` · haiku · after P5A.13. Files: `0032-container-hardening.md`.
- [ ] **P5A.27** `docs(adr): amend 0017: a third-party image and its licence` · haiku · after P5A.3. Files: `0017-dependencies-and-patches.md`. Third-party images pinned by digest with licence notices; agentgateway Apache-2.0.
- [ ] **P5A.28** `docs(adr): amend 0026: scan third-party images` · haiku · after P5A.4. Files: `0026-dependency-license-check.md`. The image scan and the gap found in P5A.4.
- [ ] **P5A.29** `docs(adr): amend 0016: tool servers as an attack surface` · haiku · after P5A.22. Files: `0016-risk-register.md`. Defend: unsigned, replayed, swapped or expired approvals refused, audit failure refuses, permission off refuses, fetch rules and private addresses (tests P5A.20 to P5A.22). Accept: a poisoned tool server or page can return hostile output; agentgateway's maturity and unverified behaviour; a compromised front can call any tool the gateway holds credentials for. Register.

Phase end: full checks, PR. The orchestrator opens it for the owner; G14, G15 and G16 stay open until each destination is approved.
