# Plan: context checks and canaries in model-gateway

Status: proposed, after 1.0. Track 3 in [plan-priorities.md](plan-priorities.md): it starts after web search and PII redaction. Nothing here is decided until the owner accepts the ADR work in Phase 0. This is the smallest version that gives the owner a canary in model calls that detects context overflow, set in the admin UI. Rules for agents: [AGENTS.md](../AGENTS.md).

Related plans: [PII detection and redaction](plan-pii-redaction.md), which builds the reply stream filter this plan extends, and [audit record viewing](plan-audit-viewer.md), which shows what this plan records.

## Summary

Askbar cannot tell today when a provider drops part of the context. `model-gateway` parses usage but never fills `StreamReport.finish_reason` (`services/model-gateway/src/llm.rs`, set to `None` near line 435 and never read in `decode_event`), so a reply cut at the output limit, or refused for an over-long context, looks like a normal reply. The server limits history by turns (`history_turns`), not by tokens. OpenAI-compatible local servers such as Ollama drop the oldest messages to fit their context and still answer with HTTP 200. The server and gateway know only the provider kind (`openai` or `anthropic`), so Ollama can't be found by kind. A limit set on the provider is the fix.

This plan adds three small layers, cheapest first:

1. **Deterministic checks (no extra model tokens):** record the provider's raw stop reason; compare the input tokens the provider reports with the gateway's estimate of what it sent.
2. **One context limit per provider,** set in the admin UI. The server trims the oldest turns to fit, so Askbar drops context on purpose rather than the provider by accident.
3. **An echo canary:** the gateway plants two random markers, one in the system message and one in the oldest message, and asks the model to repeat them at the end of its reply. The gateway strips them from the stream. If the first survives and the second doesn't, the front of the context was dropped. It runs only when the request is near the provider's limit.

A canary is a tripwire, not a guarantee. Long-context studies (RULER, NoLiMa, Chroma's Context Rot) show models lose the use of context well before the advertised window is full. That is why the help text for the limit says the effective limit is usually lower than the advertised one.

Everything else is under [Later](#later).

## Decisions

| Area | Decision |
| --- | --- |
| Where | `model-gateway` plants, checks and strips canaries. The server, clients and history never see them |
| Stop reason | `finish_reason` stays the provider's raw string (`Option<String>`), recorded on the attempt span. No enum. Values that matter: `length`, `max_tokens`, `model_context_window_exceeded` |
| Truncation check | Reported input tokens below `TRUNCATION_RATIO` (0.6) × the estimate from `sent_chars` means `context.truncated`. The constant lives in askbar-proto with its bounds; it is not a setting. Skipped when usage is estimated |
| Context limit | One nullable `context_tokens` per provider row ("Context limit" in the UI), next to `max_tokens` and with the same plumbing. Not per catalog model: `CatalogEntry` is only `{provider_id, model}` and a provider already carries one `model` |
| Trimming | The server drops the oldest turns after `last_turns` until the estimate fits `context_tokens − max_tokens`; no limit set means today's behaviour |
| Canary setting | One `canary_enabled: bool` (default true) in `GatewayConfig` |
| Canary trigger | Only when the request estimate is at or above `CANARY_TRIGGER_PCT` (70%, a named constant) of the provider's `context_tokens`. Never when no limit is set |
| Canary markers | Two nonces of 8 hex characters from the OS random source, new for each attempt; never stored, never logged |
| Placement | Marker A in the system message, marker B at the start of the oldest message sent. Ollama keeps system messages and drops old ones, so A alone would miss it |
| Echo position | At the end of the reply, so time to first token is unchanged. The reply filter holds back text that might be a marker and strips it. The filter extends the PII filter struct (PII-P2.2 in [plan-pii-redaction.md](plan-pii-redaction.md)), which ships first in track 2; no trait or chain |
| Outcomes | `ok`, `front_lost`, `absent` (inconclusive: the model ignored the instruction), `skipped` (output limit cut the marker). Always recorded. A canary never fails a finished reply |
| Audit | A `step` span named `canary`, written by `model-gateway` under the attempt ([audit viewer](plan-audit-viewer.md) AV2.1, which lands in track 1). `attrs.canary = {planted, outcome, instruction_version}`: booleans and counts, never the nonces. The record shows that the model saw extra text, because the server's sealed request does not contain it |
| Alerts | Only `ContextTruncated`, on `front_lost` or `truncated`. `absent` never alerts |
| Naming | "Canary" or "marker", never "canary token": in Askbar a token is a bearer secret ([glossary](architecture.md#glossary)) |

## Config

In `crates/askbar-proto/src/gateway.rs`, a `#[serde(default)]` field pushed through the existing `PUT /config`. The config version hash covers it.

```rust
pub canary_enabled: bool,      // default true
```

In `crates/askbar-proto/src/lib.rs`, named constants with their bounds:

```rust
pub const TRUNCATION_RATIO: f32 = 0.6;
pub const CANARY_TRIGGER_PCT: u8 = 70;
```

`context_tokens: Option<u32>` is added to `SnapshotProvider` and the gateway's provider config, the same way as `max_tokens`. The server gets it in its snapshot so it can trim.

## Admin UI

- **Providers:** a "Context limit" number field on each provider, with help text that the effective limit is usually much lower than the advertised one.
- **Settings page (existing):** one checkbox, "Context canary".
- **Model calls list:** a context column (`ok`, `truncated`, `front lost`) from span attributes.

Every change is an admin action record, as today.

## Alerts

One new `Kind` in `crates/askbar-proto/src/alerter.rs`: `ContextTruncated`. `fed_of` in `services/auditor/src/rules.rs` maps only `AdminAction`, `Hello` and turn `Start`, so alerting on finish attributes needs a new `Fed` path for span finish and step records. For rate limits, check `services/alerter/src/limits.rs` first: it already allows one delivery per derived key per hour and 20 an hour in all, and the key is built from typed fields, so see whether a model or provider field fits before adding anything custom.

## Owner gates

- **Phase 0:** accept the ADR work. Canaries are a `model-gateway` behaviour, so they amend [ADR 0054](adr/0054-model-gateway.md) and [ADR 0014](adr/0014-model-gateway-and-providers.md) in place rather than taking a new number ([ADR 0001](adr/0001-decision-records-and-change-rules.md)). The audit attributes and the new alert kind amend [ADR 0051](adr/0051-auditor.md) and [ADR 0055](adr/0055-alerter.md) (C0.2, only when Phase 3 starts).
- Alerts fire only on `front_lost` or truncated, so there is no record-only period. Watch the `absent` rate for each model in the model calls list.

## Phases

| Phase | Content | Status |
| --- | --- | --- |
| 0 | ADR amendments (0054 and 0014; 0051 and 0055 for `ContextTruncated`) | not started |
| 1 | Raw stop reason and input-count check, record only | not started |
| 2 | Provider context limit and server trimming | not started |
| 3 | Echo canary, `ContextTruncated` alert and the settings checkbox | not started |
| Later | See below | not started |

### Phase 0: records

- [ ] **C0.1** `docs(adr): amend 0054 and 0014: context checks and canaries` · sonnet
  Files: `docs/adr/0054-model-gateway.md`, `docs/adr/0014-model-gateway-and-providers.md`.
  Do: add the Decisions table above as rules; list what a canary is not (proof of context use, an injection defence).
  Accept: `scripts/check-docs.sh` passes; each ADR's Amendments section names this plan.
- [ ] **C0.2** `docs(adr): amend 0051 and 0055: canary attributes and ContextTruncated` · haiku · after C0.1
  Files: `docs/adr/0051-auditor.md`, `docs/adr/0055-alerter.md`, `docs/reference/audit/model-calls.md`, `docs/reference/audit/alert-kinds.md`.
  Accept: the reference pages list each new attribute and the alert kind; `scripts/check-reference.sh` passes.

### Phase 1: stop reason and input count (record only)

- [ ] **C1.1** `fix(model-gateway): record the provider's finish reason` · haiku
  Files: `services/model-gateway/src/llm.rs`, its tests.
  Do: set `finish_reason` in `decode_event` from OpenAI `choices[].finish_reason` and Anthropic `message_delta.delta.stop_reason`, as the raw string. No enum.
  Accept: unit tests for `stop`, `length`, `end_turn`, `max_tokens`, `model_context_window_exceeded` and a missing reason; the attempt span's `finish_reason` is no longer null in `tests/streaming.rs`.
- [ ] **C1.2** `feat(model-gateway): compare reported input tokens with the estimate` · sonnet · after C1.1
  Files: `services/model-gateway/src/{lib,llm}.rs`, `crates/askbar-proto/src/lib.rs`, `tests/e2e/mock_llm.py`, `tests/e2e/mock_llm_test.py`, tests.
  Do: estimate input tokens from `sent_chars` (already computed in `stream_chat_report`); after the reply set, on the attempt span, `attrs.context = {estimated_in, reported_in, truncated, stop}`. `truncated` uses `TRUNCATION_RATIO`. Skip when usage is itself estimated. Add a mock-LLM mode that drops the oldest messages to a size and reports the smaller input count, like Ollama ([ADR 0043](adr/0043-mock-llm.md)).
  Accept: a mock-LLM test in truncation mode gives `truncated: true`; a normal reply gives `false`; nothing changes in routing or the reply.

### Phase 2: provider context limit and trimming

- [ ] **C2.1** `feat(admin): add a context limit to providers` · sonnet · after C1.1
  Files: `crates/askbar-proto/src/lib.rs`, `services/admin/src/{db,admin_api,gateway_link,server_link}.rs`, `services/admin/src/ui/templates/providers.html`, `services/admin/src/ui/app.js`, `docs/openapi/admin.v1.yaml`, admin browser test.
  Do: `context_tokens: Option<u32>` on `SnapshotProvider`, the gateway provider config and the provider row, with the same plumbing as `max_tokens`: an admin db migration adding a nullable column, the snapshot and the gateway config. A "Context limit" field in the providers form, with the help text that the effective limit is usually lower than advertised. Bounds next to the field.
  Accept: old config JSON parses unchanged; the OpenAPI test passes; a push carries the value; the field saves and reloads in the browser test.
- [ ] **C2.2** `feat(server): trim history to the provider's context limit` · sonnet · after C2.1
  Files: `services/server/src/{llm,generation}.rs`, tests.
  Do: after `last_turns` in `generation.rs`, drop the oldest turns until the estimate fits `context_tokens − max_tokens`; always keep the system prompt and the newest message; record `trimmed_turns` on the turn span.
  Accept: a test with a small limit sends fewer turns and records the count; no limit set means today's behaviour.

### Phase 3: echo canary

- [ ] **C3.1** `feat(model-gateway): plant and check the context canary` · sonnet · after C2.1 and PII-P2.2
  Files: `crates/askbar-proto/src/gateway.rs`, `services/model-gateway/src/{lib,filter,canary}.rs`, tests.
  Do: add `canary_enabled` and `CANARY_TRIGGER_PCT`. When enabled, a limit is set and the estimate is at or above the trigger, plant A and B and the instruction. Extend the PII filter struct to hold back and strip marker text. Write `attrs.canary = {planted, outcome, instruction_version}` on a `canary` step span under the attempt (AV2.1).
  Accept: property tests split markers at every byte boundary and the client never receives a partial marker; mock-LLM tests for each outcome; no canary when no limit is set or below 70%; nonces appear in no log or attribute (a test greps for them).
- [ ] **C3.2** `feat(auditor): alert on truncated context` · sonnet · after C3.1 and AV2.1
  Files: `crates/askbar-proto/src/alerter.rs`, `services/auditor/src/{rules,lib}.rs`, tests.
  Do: add a new `Fed` path for span finish and step records, since `fed_of` maps only `AdminAction`, `Hello` and turn `Start`; raise `ContextTruncated` on `front_lost` or `context.truncated`. Check `services/alerter/src/limits.rs` first and reuse its limits if they fit.
  Accept: `front_lost` or `truncated` raises one alert within the limits; `absent` does not alert.
- [ ] **C3.3** `feat(ui): context canary checkbox` · haiku · after C3.1
  Files: admin settings template, `app.js`, `admin_api.rs`, `docs/openapi/admin.v1.yaml`, admin browser test.
  Accept: the checkbox saves, pushes and shows in the gateway's status.

### Later

- **Leak tripwire, decoy credentials, scheduled audit canary, drift probes:** a system prompt leak tripwire; decoy admin credentials (note: `RejectedToken` is raised by the admin's notifier, `services/admin/src/notify.rs`, not the auditor, so decoys belong there); a scheduled audit canary (`ChainMismatch` and `ComponentSilent` already cover most of it); drift probes if a model is ever swapped silently.
- **Injection markers in fetched pages** (web search ships first; needs the echo canary): a per-page nonce in each fetched page's untrusted block; if a reply or proposed fetch contains it, drop the action and alert. Don't rely on a detector model asked to repeat a secret: the DataFlip attack (2025) defeats it.
- **Cross-space honeytokens** (with memory): one decoy entry per space; its appearance in another space's request or reply is a space isolation breach.
- **Canary modes, actions and a test button:** `off`/`always` modes, `alert_and_mark`, a `trigger_pct` setting, a Canaries settings section and `POST /canary/test`, if the single checkbox proves too coarse.
- **Per-model limits and an effective-context pair:** `context_window` and `effective_context` on each approved model, if one provider ever serves several models.

## Risks

- **False alarms:** models ignore the echo instruction some of the time. `absent` never alerts; watch its rate in the model calls list.
- **Reply quality:** the instruction costs about 40 input tokens and one line of output. The 70% trigger limits it to long contexts.
- **Prompt caching:** markers change every attempt. Plant marker B after the cached prefix, or skip the canary on providers where cache hits matter more; measure in Phase 3.
- **Wrong limit:** a limit set too high means no trim and no canary trigger. The help text says to set it below the advertised window.

## Sources

- [RULER](https://arxiv.org/abs/2404.06654) · [NoLiMa](https://arxiv.org/html/2502.05167v3) · [Context Rot (Chroma)](https://trychroma.com/research/context-rot)
- [Ollama's silent context truncation](https://baeseokjae.github.io/posts/ollama-silent-context-truncation-2026/)
- [Anthropic: handling stop reasons](https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons)
- [Evaluation of early detection systems, including Rebuff's canary](https://arxiv.org/html/2506.19109v1)
- [How Not to Detect Prompt Injections with an LLM](https://arxiv.org/html/2507.05630v2)
- [Log Probability Tracking of LLM APIs](https://arxiv.org/pdf/2512.03816) · [Auditing Model Substitution in LLM APIs](https://arxiv.org/abs/2504.04715v2)
