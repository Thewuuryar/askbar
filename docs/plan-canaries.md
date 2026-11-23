# Plan: context checks and canaries in model-gateway

Status: proposed, after 1.0. Track 3 in [plan-priorities.md](plan-priorities.md): it starts after web search and PII redaction. Nothing here is decided until the owner accepts the ADR work in Phase 0. Rules for agents: [AGENTS.md](../AGENTS.md).

Related plans: [PII detection and redaction](plan-pii-redaction.md), which builds the reply stream filter this plan extends, and [audit record viewing](plan-audit-viewer.md), which shows what this plan records.

## Summary

Askbar cannot tell today when a provider drops part of the context. `model-gateway` parses usage but never fills `StreamReport.finish_reason` (`services/model-gateway/src/llm.rs`), so a reply cut at the output limit, or refused for an over-long context, looks like a normal reply. The server limits history by turns (`history_turns`), not by model tokens. OpenAI-compatible local servers such as Ollama drop the oldest messages to fit their context and still answer with HTTP 200.

This plan adds three layers, cheapest first:

1. **Deterministic checks (no extra model tokens):** record the stop reason; compare the input tokens the provider reports with the gateway's estimate of what it sent; trim history on the server to a per-model context limit, so Askbar drops context on purpose rather than the provider by accident.
2. **An echo canary:** the gateway plants two random markers, one in the system message and one in the oldest message, and asks the model to repeat them at the end of its reply. The gateway strips them from the stream. If the first survives and the second doesn't, the front of the context was dropped.
3. **Other canaries:** a system prompt leak tripwire, decoy credentials, an end-to-end audit canary, and model drift probes. Cross-space honeytokens and injection markers wait for memory and web research.

A canary is a tripwire, not a guarantee. Long-context studies (RULER, NoLiMa, Chroma's Context Rot) show models lose the use of context well before the advertised window is full, so a marker that survives doesn't prove the model used what was around it. That is why each model also gets an **effective context** limit, below its advertised window.

## Decisions

| Area | Decision |
| --- | --- |
| Where | `model-gateway` plants, checks and strips canaries. The server, clients and history never see them |
| Stop reason | Normalised to `complete`, `output_limit`, `context_exceeded`, `filtered`, `other`, recorded on every attempt span |
| Truncation check | Reported input tokens below `truncation_ratio` × the gateway's estimate (default 0.6) means `context.truncated` |
| Context limits | Each approved model gains `context_window` and `effective_context`; the server trims oldest turns to `effective_context − max_tokens` |
| Canary markers | Two nonces of 8 hex characters from the OS random source, new for each attempt; never stored, never logged |
| Placement | Marker A in the system message, marker B at the start of the oldest message sent. Ollama keeps system messages and drops old ones, so A alone would miss it |
| Echo position | At the end of the reply, so time to first token is unchanged. The gateway holds back text that might be a marker and strips it |
| Outcomes | `ok`, `front_lost`, `absent` (inconclusive: the model ignored the instruction), `skipped` (output limit cut the marker) |
| Modes | `off`, `near_limit` (default: only above `trigger_pct` of `effective_context`, default 70%), `always` |
| Actions | `record`, `alert` (default), `alert_and_mark` (adds a notice that earlier messages may have been dropped). A canary never fails a finished reply |
| Audit | Attempt span `attrs.context` and `attrs.canary`: booleans and counts, never the nonces. `canary.planted` and the instruction version record that the model saw extra text, because the server's sealed request does not contain it |
| Naming | "Canary" or "marker", never "canary token": in Askbar a token is a bearer secret ([glossary](architecture.md#glossary)) |

## Config

Added to `GatewayConfig` in `crates/askbar-proto/src/gateway.rs` as `#[serde(default)]` fields, pushed through the existing `PUT /config`. The config version hash covers them.

```rust
pub struct CanaryConfig {
    pub context_mode: CanaryMode,       // off | near_limit | always (default near_limit)
    pub context_trigger_pct: u8,        // default 70, range 10..=100
    pub context_action: CanaryAction,   // record | alert | alert_and_mark (default alert)
    pub truncation_ratio: f32,          // default 0.6, range 0.1..=0.95
    pub leak_enabled: bool,             // default true
    pub leak_action: LeakAction,        // record | alert | redact (default redact)
    pub drift_probe_hours: Option<u16>, // None = off
}
// CatalogEntry gains: context_window: Option<u32>, effective_context: Option<u32>
```

The server receives `context_window`, `effective_context` and the provider's `max_tokens` in its snapshot so it can trim.

## Admin UI

- **Approved models:** "Context window" and "Effective context" number fields on each row, with help text explaining that the effective limit is usually much lower than the advertised one.
- **Settings, new "Canaries" section:** context check mode, trigger %, action, truncation ratio, leak tripwire on or off and its action, drift probe interval, and a **Send test canary** button that runs one canary turn against a chosen model and shows the outcome.
- **Model calls list:** a context column (`ok`, `truncated`, `front lost`, `at limit`) taken from span attributes.

Every change is an admin action record, as today.

## Alerts

New `Kind` values in `crates/askbar-proto/src/alerter.rs`: `ContextTruncated`, `CanaryLeak`, `ModelDrift`, `DecoyCredentialUsed`. The auditor's rules read attempt-finish attributes (`fed_of` in `services/auditor/src/rules.rs`) and send at most one alert per model per hour for each kind.

## Owner gates

- **Phase 0:** accept the ADR work. Canaries are a `model-gateway` behaviour, so they amend [ADR 0054](adr/0054-model-gateway.md) and [ADR 0014](adr/0014-model-gateway-and-providers.md) in place rather than taking a new number ([ADR 0001](adr/0001-decision-records-and-change-rules.md)). The audit attributes amend [ADR 0051](adr/0051-auditor.md); the new alert kinds amend [ADR 0055](adr/0055-alerter.md).
- **Before Phase 3 alerts are switched on:** two weeks of `record` mode, then the owner reviews the `absent` rate for each model.

## Phases

| Phase | Content | Status |
| --- | --- | --- |
| 0 | ADR amendments, reference docs | not started |
| 1 | Stop reason and input-count check, record only | not started |
| 2 | Per-model context limits and server trimming | not started |
| 3 | Echo canary and the shared stream filter | not started |
| 4 | Leak tripwire, decoy credentials, audit canary | not started |
| 5 | Drift probes | not started |
| 6 | Injection markers in fetched pages (web search ships first) | not started |
| Later | Cross-space honeytokens (with memory) | not started |

### Phase 0: records

- [ ] **C0.1** `docs(adr): amend 0054 and 0014: context checks and canaries` · sonnet
  Files: `docs/adr/0054-model-gateway.md`, `docs/adr/0014-model-gateway-and-providers.md`.
  Do: add the Decisions table above as rules; list what a canary is not (proof of context use, an injection defence).
  Accept: `scripts/check-docs.sh` passes; each ADR's Amendments section names this plan.
- [ ] **C0.2** `docs(adr): amend 0051 and 0055: canary attributes and alert kinds` · haiku · after C0.1
  Files: `docs/adr/0051-auditor.md`, `docs/adr/0055-alerter.md`, `docs/reference/audit/model-calls.md`, `docs/reference/audit/alert-kinds.md`.
  Accept: the reference pages list each new attribute and alert kind; `scripts/check-reference.sh` passes.

### Phase 1: stop reason and input count (record only)

- [ ] **C1.1** `fix(model-gateway): record the provider's stop reason` · haiku
  Files: `services/model-gateway/src/llm.rs`, its tests.
  Do: set `finish_reason` in `decode_event` from OpenAI `choices[].finish_reason` and Anthropic `message_delta.delta.stop_reason`; add `StopKind` with the five normalised values.
  Accept: unit tests for `stop`, `length`, `end_turn`, `max_tokens`, `model_context_window_exceeded` and a missing reason; the attempt span's `finish_reason` is no longer null in `tests/streaming.rs`.
- [ ] **C1.2** `feat(model-gateway): compare reported input tokens with the estimate` · sonnet · after C1.1
  Files: `services/model-gateway/src/{lib,llm}.rs`, tests.
  Do: estimate input tokens from the exact request body before sending (reuse `estimate_tokens`); after the reply set `attrs.context = {estimated_in, reported_in, truncated, at_limit, stop}`. `truncated` uses `truncation_ratio`; `at_limit` uses `context_window` when known. Skip when usage is itself estimated.
  Accept: a mock-LLM test that reports a low `prompt_tokens` gives `truncated: true`; a normal reply gives `false`; nothing changes in routing or the reply.
- [ ] **C1.3** `test(mock-llm): truncate the front of the context on request` · haiku · after C1.2
  Files: `tests/e2e/mock_llm.py`, `tests/e2e/mock_llm_test.py`.
  Do: a mock mode that drops the oldest messages to a size and reports the smaller input count, like Ollama ([ADR 0043](adr/0043-mock-llm.md)).
  Accept: the gateway test from C1.2 uses it end to end.

### Phase 2: context limits and trimming

- [ ] **C2.1** `feat(proto): add context limits to approved models` · haiku
  Files: `crates/askbar-proto/src/{lib,gateway}.rs`, tests.
  Do: `context_window` and `effective_context` on `CatalogEntry`, optional; bounds and defaults next to the field.
  Accept: old config JSON parses unchanged; the new fields round-trip.
- [ ] **C2.2** `feat(admin): store and push model context limits` · sonnet · after C2.1
  Files: `services/admin/src/{db,catalog,gateway_link,server_link,admin_api}.rs`, `docs/openapi/admin.v1.yaml`.
  Do: append a migration adding the two columns to `catalog_models`; validate `effective_context ≤ context_window`; include both in the gateway config and the server snapshot.
  Accept: the OpenAPI test passes; a push carries the values; an invalid pair is refused with an `ErrorCode`.
- [ ] **C2.3** `feat(server): trim history to the model's effective context` · sonnet · after C2.2
  Files: `services/server/src/{llm,generation}.rs`, tests.
  Do: after `last_turns`, drop the oldest turns until the estimate fits `effective_context − max_tokens`; always keep the system prompt and the newest message; record `trimmed_turns` on the turn span.
  Accept: a test with a small limit sends fewer turns and records the count; no limit set means today's behaviour.
- [ ] **C2.4** `feat(ui): context limit fields on approved models` · haiku · after C2.2
  Files: `services/admin/src/ui/templates/providers.html`, `services/admin/src/ui/app.js`, admin browser test.
  Accept: the fields save and reload; the browser test covers them.

### Phase 3: echo canary

- [ ] **C3.1** `feat(proto): add canary settings` · haiku
  Files: `crates/askbar-proto/src/gateway.rs`.
  Accept: `CanaryConfig` with the defaults above; old config parses.
- [ ] **C3.2** `feat(model-gateway): add the canary marker filter` · sonnet · after C3.1 and PII-P2.2
  Files: `services/model-gateway/src/{filter,canary}.rs`, tests.
  Do: add a marker filter to the `StreamFilter` chain built in [plan-pii-redaction.md](plan-pii-redaction.md) (PII-P2.2), after PII restore.
  Accept: property tests split markers at every byte boundary and the client never receives a partial marker.
- [ ] **C3.3** `feat(model-gateway): plant and check the context canary` · sonnet · after C3.2
  Files: `services/model-gateway/src/{lib,canary}.rs`, tests.
  Do: in `near_limit` or `always` mode, plant A and B and the instruction; strip the echo with the filter; set `attrs.canary = {planted, outcome, instruction_version}`; apply `context_action`.
  Accept: mock-LLM tests for each outcome; nonces appear in no log or attribute (a test greps for them).
- [ ] **C3.4** `feat(auditor): alert on truncated context` · haiku · after C3.3
  Files: `crates/askbar-proto/src/alerter.rs`, `services/auditor/src/rules.rs`, tests.
  Accept: `front_lost` or `truncated` raises `ContextTruncated` once per model per hour; `absent` does not alert.
- [ ] **C3.5** `feat(ui): canary settings and test button` · sonnet · after C3.3
  Files: admin settings template, `app.js`, `admin_api.rs`, a gateway admin route `POST /canary/test`, both OpenAPI files.
  Accept: settings save, push and show in the gateway's status; the test button shows the outcome.

### Phase 4: leak tripwire, decoys, audit canary

- [ ] **C4.1** `feat(model-gateway): system prompt leak tripwire` · sonnet
  Do: a separate nonce in the system message marked confidential; the filter redacts or records it if it appears in a reply; `CanaryLeak` alert.
  Accept: a mock reply that echoes the system prompt is caught; normal replies are unaffected.
- [ ] **C4.2** `feat(admin): decoy admin API key and pairing key` · sonnet
  Do: create one decoy of each at first run; they never authorise anything; any use records an admin action and raises `DecoyCredentialUsed` (same path as `RejectedToken`, [ADR 0024](adr/0024-rejected-token-alert.md)).
  Accept: a request with the decoy gets the normal refusal and an alert.
- [ ] **C4.3** `feat(admin): scheduled audit canary` · sonnet
  Do: daily, a synthetic turn through the mock provider checks that the span opens, the payload is sealed, the chain grows and the alert path answers. Never counts toward a budget. Off when no mock provider is configured.
  Accept: breaking any of those steps in a test raises an alert.

### Phase 5: drift probes

- [ ] **C5.1** `feat(model-gateway): scheduled drift probes` · sonnet
  Do: every `drift_probe_hours`, send a fixed small probe set to each approved metered model. Where the provider returns log-probabilities, compare against a stored baseline with a permutation test (Log Probability Tracking, 2025); otherwise compare answer fingerprints. Record a `probe` attempt under a dedicated turn; tokens count toward a separate small budget.
  Accept: a mock provider that switches models mid-run raises `ModelDrift`; cost per probe is under 100 tokens.

### Phase 6: injection markers in fetched pages

Web search (track 1) already datamarks fetched text as untrusted. This phase adds a tripwire on top.

- [ ] **C6.1** `feat(server): plant canaries in fetched text` · sonnet · after C3.3
  Do: put a per-page nonce inside each fetched page's untrusted block; if a reply, a proposed query or a proposed fetch URL contains it, the page steered the model: drop the action, record `injection_suspected` on the tool span, and alert. Don't rely on a detector model that is asked to repeat a secret: the DataFlip attack (2025) defeats it.
  Accept: a fixture page that tells the model to repeat the hidden text or fetch a URL is flagged; ordinary pages are not.

### Later

- **Cross-space honeytokens** (with memory): one decoy entry per space; its appearance in another space's request or reply is a space isolation breach.

## Risks

- **False alarms:** models ignore the echo instruction some of the time. `absent` never alerts, and Phase 3 starts in `record` mode.
- **Reply quality:** the instruction costs about 40 input tokens and one line of output. `near_limit` mode limits it to long contexts.
- **Prompt caching:** markers change every attempt. Plant marker B after the cached prefix, or skip the canary on providers where cache hits matter more; measure in Phase 3.

## Sources

- [RULER](https://arxiv.org/abs/2404.06654) · [NoLiMa](https://arxiv.org/html/2502.05167v3) · [Context Rot (Chroma)](https://trychroma.com/research/context-rot)
- [Ollama's silent context truncation](https://baeseokjae.github.io/posts/ollama-silent-context-truncation-2026/)
- [Anthropic: handling stop reasons](https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons)
- [Evaluation of early detection systems, including Rebuff's canary](https://arxiv.org/html/2506.19109v1)
- [How Not to Detect Prompt Injections with an LLM](https://arxiv.org/html/2507.05630v2)
- [Log Probability Tracking of LLM APIs](https://arxiv.org/pdf/2512.03816) · [Auditing Model Substitution in LLM APIs](https://arxiv.org/abs/2504.04715v2)
