# Plan: PII detection and redaction in model-gateway

Previous: [Tiered routing risks](plan-tiered-routing/risks.md) · Next: [Canaries](plan-canaries.md) · Tracker: [task-tracker.md](task-tracker.md)

Status: proposed, after 1.0. Track 2 in [plan-priorities.md](plan-priorities.md): it starts after web search. Nothing here is decided until the owner accepts the ADR work in Phase 0. Rules for agents: [AGENTS.md](../AGENTS.md).

Related plans: [context checks and canaries](plan-canaries.md), which adds to the reply stream filter this plan builds, and [audit record viewing](plan-audit-viewer.md), which shows what this plan records.

## Summary

`model-gateway` is the last hop before a provider, so it detects personal data in each request and replaces it with a placeholder such as `⟦PERSON_1⟧`. As the reply streams back, the gateway swaps placeholders back to the real values, so the user never sees them. Providers see placeholders; the user's history keeps what they typed.

Detection is in process: patterns with checksums for structured data (email, phone, card numbers with Luhn, IBAN with mod-97, national ID numbers, IP addresses) and secrets people paste (provider API keys, GitHub tokens, JWTs, private keys). Pure Rust, well under a millisecond. It ships in `detect_only` mode first.

Detection is a filter, not a guarantee. On hard real-world text the best small models score an F1 of about 0.4–0.5, far below the 0.8–0.96 their authors report on synthetic data. Patterns do not find names or addresses at all. Indirect identifiers ("my son's teacher at the school on Elm Street") pass through. The admin documentation must say so.

## Decisions

| Area | Decision |
| --- | --- |
| Where | A module, `services/model-gateway/src/pii.rs`. No new crate. [PII-P2.6](#phase-2-pseudonymize-and-restore) moves it into a crate only when that step starts |
| Injection point | In `run_attempts` (`services/model-gateway/src/lib.rs`), once before the retry loop, on `messages` and `req.system`. Retries reuse the result |
| Order | Detection runs before `gates()`, so a blocked type can be refused |
| Placeholder | `⟦TYPE_n⟧`, numbered per request by first occurrence of each normalised value within a type: `⟦PERSON_1⟧`, `⟦PERSON_2⟧`, `⟦EMAIL_1⟧` |
| Stability | `ChatRequest` carries the whole history on every call (`crates/askbar-proto/src/gateway.rs`), so numbering is the same on each turn while the history is unchanged. `conversation_id` is optional, so nothing is keyed on it. No secret, no cache |
| Reverse map | Built for each request from what that request contained. Nothing is stored |
| Restore | A plain filter struct in the delta closure of `run_attempts`: `delta_tx.send(filter.push(d))`, and `finish()` after `stream_chat_report` returns. It holds back text from `⟦` until `⟧` or 24 characters, then substitutes or passes it through. It also accepts the placeholder without brackets. On cancel or error it flushes the held-back text and never drops it. [Canaries](plan-canaries.md) extend the struct later |
| Actions | `pseudonymize` (restored in the reply), `block` (turn refused at a gate), `allow` |
| Default actions | `block`: secrets, private keys, card numbers. `pseudonymize`: email, phone, IBAN, national ID. `allow`: everything else |
| Mode | `detect_only` (default: count, change nothing) or `enforce` |
| Code blocks | Inside fenced code, secrets only. Hard-coded: identifiers, hashes and UUIDs are the main false positives |
| Block | Needs a new `ErrorCode` in `askbar-proto`, an entry in `docs/openapi/model-gateway.v1.yaml`, and a refusal in `gates()`. The refusal names the type only |
| Audit | The server still seals the request as typed. [Audit viewer](plan-audit-viewer.md) AV2.3 (track 1) seals `provider_request`, what the provider actually received, on the attempt span. This plan adds a `redact` step span (kind `step`, written by model-gateway under the attempt) with unsealed `attrs.pii`: counts by type and action, and the `restored` count. Never values |
| Terms lists | Always-redact and never-redact terms are themselves personal data: stored in `admin.db`, never in alerts or audit attributes |

## Config

Added to `GatewayConfig` in `crates/askbar-proto/src/gateway.rs` as `#[serde(default)]`, pushed through `PUT /config`.

```rust
pub struct PiiConfig {
    pub enabled: bool,
    pub mode: PiiMode,                          // detect_only (default) | enforce
    pub entities: BTreeMap<String, PiiAction>,  // type -> pseudonymize | block | allow
    pub allow_terms: Vec<String>,
    pub deny_terms: Vec<String>,                // always pseudonymized as CUSTOM; not in `entities`
}
```

The admin keeps it as one JSON settings row in `admin.db` and adds it in `build_config` (`services/admin/src/gateway_link.rs`). No new tables.

## Admin UI: a new Privacy page

1. On or off, and the mode.
2. An entity table: one row per type with an action.
3. The two term lists, one term per line.

## Owner gates

- **Phase 0:** accept the ADR work.
  - The placeholder scheme and the detection module amend [ADR 0054](adr/0054-model-gateway.md) and [ADR 0014](adr/0014-model-gateway-and-providers.md).
  - The `provider_request` payload amends [ADR 0011](adr/0011-sealed-content-access.md) and [ADR 0051](adr/0051-auditor.md), if AV2.3 has not already done it.
- **Stop and ask, Phase 1:** the `regex` crate is not yet a direct dependency anywhere. Adding it needs the owner and the licence check in [ADR 0026](adr/0026-dependency-license-check.md).
- **Before Phase 2:** two weeks in `detect_only`, then the owner reviews the counts for false positives in the existing trace view (Audit → Model calls), which shows span attributes; the conversation view comes later, in track 5.

## Phases

| Phase | Content | Status |
| --- | --- | --- |
| 0 | ADR amendments | not started |
| 1 | Detection module, `detect_only` | not started |
| 2 | Pseudonymize, restore, block, `redact` step | not started |
| 3 | Admin Privacy page | not started |

### Phase 0: records

- [x] **PII-P0.1** `docs(adr): amend 0054 and 0014: PII detection and placeholders` · sonnet
  Accept: the Decisions table is recorded as rules; the limits paragraph from the Summary is included.
- [x] **PII-P0.2** `docs(adr): amend 0011 and 0051: provider request payload and PII attributes` · haiku · after PII-P0.1
  Files: the two ADRs, `docs/reference/audit/model-calls.md`.
  Do: skip the `provider_request` part if AV2.3 already recorded it. One amendment per commit, so split into two commits if the repo rule needs it.

### Phase 1: detect only

- [x] **PII-P1.1** `feat(proto): add PII settings` · haiku
  Files: `crates/askbar-proto/src/gateway.rs`. Accept: defaults as above; old config parses.
- [x] **PII-P1.2** `feat(model-gateway): detect PII and record counts` · sonnet · after PII-P1.1, owner approval of the `regex` dependency
  Files: `services/model-gateway/src/{lib,pii}.rs`, `services/model-gateway/Cargo.toml`, tests.
  Do: `detect(text, &PiiConfig) -> Vec<Span>` with types and byte ranges; validators for Luhn and IBAN; secret rules; secrets-only inside fenced code; allow and deny terms; merge overlaps, preferring validated spans. In `run_attempts`, run it once before the retry loop on `messages` and `req.system`; open a `redact` step with `attrs.pii = {counts, actions, mode}`; send nothing different to the provider.
  Accept: table tests per type, including false-positive cases (UUIDs, git hashes, version numbers, the names Will, Rose and Mark); counts appear on the `redact` step; a test greps logs and attributes for a planted email and finds nothing.

### Phase 2: pseudonymize and restore

- [x] **PII-P2.1** `feat(model-gateway): replace PII with placeholders` · sonnet
  Files: `services/model-gateway/src/{lib,pii}.rs`.
  Do: in `run_attempts`, once before the retry loop, redact `messages` and `req.system` in `enforce` mode, numbering by first occurrence as in Decisions. After redaction, append one short line saying placeholders stand for private values and should be reused as written.
  Accept: the same value gives the same placeholder on two turns with the same history; two requests with the same text give the same result; `detect_only` changes nothing.
- [ ] **PII-P2.2** `feat(model-gateway): add the reply filter and restore placeholders` · sonnet · after PII-P2.1
  Files: `services/model-gateway/src/{lib,pii}.rs`, tests.
  Do: a plain filter struct (`push(&str) -> String`, `finish() -> String`), no trait or chain. Call `delta_tx.send(filter.push(d))` in the delta closure and `finish()` after `stream_chat_report` returns. On cancel or error, flush held-back text; never drop it. Restore from the per-request reverse map.
  Accept: property tests split placeholders at every byte; unknown placeholders pass through unchanged; a cancelled stream loses no text.
- [ ] **PII-P2.3** `feat(model-gateway): refuse a turn with blocked PII` · haiku · after PII-P2.1
  Files: `crates/askbar-proto`, `docs/openapi/model-gateway.v1.yaml`, `services/model-gateway/src/lib.rs`.
  Do: a new `ErrorCode`, its entry in the OpenAPI file, and a refusal in `gates()`. Detection runs before `gates()`. The server returns it as `application/problem+json` ([ADR 0023](adr/0023-api-error-format.md)). The refusal attrs carry the type only.
  Accept: a pasted `sk-ant-` key is refused, and the refusal carries no part of it.
- [ ] **PII-P2.4** `feat(model-gateway): record the redact step` · haiku · after PII-P2.1, PII-P2.2, audit viewer AV2.3
  Do: AV2.3 seals `provider_request` on the attempt span. Check it holds the redacted text. The `redact` step span (kind `step`, under the attempt) gets counts by type and action, and the `restored` count in its attrs.
  Accept: the payload decrypts in the sealed view to the redacted text; the `redact` step shows counts and `restored`, no values.
- [ ] **PII-P2.6** `feat(mcp-gateway): check search queries and fetch URLs for PII` · sonnet · after PII-P1.2
  Files: the MCP gateway front's search and fetch handlers, tests.
  Do: first move `pii.rs` into a crate shared with the MCP gateway front. Then run the tier 1 detector on each search query and fetch URL before the audit span. Secrets refuse the call; other PII is removed from the query, the call goes ahead, and the reply says a term was left out. Record counts on the tool span, never values. Placeholders are no use to a search engine, so queries are never pseudonymized.
  Accept: a query containing an email address reaches the fixture search engine without it; a query with a provider key is refused.

### Phase 3: admin

- [ ] **PII-P3.1** `feat(admin): store and push PII settings` · sonnet
  Files: `services/admin/src/{gateway_link,admin_api}.rs`, `docs/openapi/admin.v1.yaml`.
  Do: one JSON settings row in `admin.db`, added to the config in `build_config`; no new tables.
- [ ] **PII-P3.2** `feat(ui): Privacy page` · sonnet · after PII-P3.1
  Files: `services/admin/src/pages.rs`, a new template, `app.js`, admin browser test.
  Accept: on/off, mode, the entity table and both term lists work; text is written with `textContent` only. The page states the honest limits from the Summary.

## Later

- Tier 2 named-entity detector (GLiNER sidecar, own container and ADR): only if detect_only counts show names or addresses leaking. Model: `knowledgator/gliner-pii-base-v1.0`, UINT8 ONNX, Apache-2.0. Not Piiranha (CC-BY-NC-ND forbids modification). OpenAI Privacy Filter is the other candidate.
- Surrogates: realistic fake values instead of placeholders, only if answers suffer.
- Try it route: a gateway admin route that shows the spans found in pasted text.
- 7-day counts on the Privacy page.
- Reply scanning for PII in model output.
- SecretInPrompt warning alert.
- Per-user overrides, custom patterns, presets, a `mask` action and a minimum score.

## Risks

- **Answer quality:** placeholders hurt questions about the value itself ("is this address valid?"). The `allow` action covers it.
- **Missed names and addresses:** patterns do not find them. The `detect_only` counts and the tier 2 Later line are the check.
- **Split identifiers:** a name in one message and an address in another are each judged alone.
- **Edited history:** if the user edits an earlier message, first-occurrence numbers may shift. Only that turn is affected.

## Sources

- [OpenAI Privacy Filter](https://openai.com/index/introducing-openai-privacy-filter/) · [model card](https://huggingface.co/openai/privacy-filter)
- [knowledgator/gliner-pii-base-v1.0](https://huggingface.co/knowledgator/gliner-pii-base-v1.0)
- [GLiNER2-PII and the SPY benchmark](https://arxiv.org/pdf/2605.09973)
- [Piiranha licence](https://huggingface.co/iiiorg/piiranha-v1-detect-personal-information)
- [SurrogateShield](https://arxiv.org/pdf/2606.29567) · [Anonymous-by-Construction](https://arxiv.org/html/2603.17217)
- [LiteLLM PII masking](https://docs.litellm.ai/docs/proxy/guardrails/pii_masking_v2) · [Kong AI Sanitizer](https://developer.konghq.com/plugins/ai-sanitizer/) · [Portkey PII redaction](https://portkey.ai/docs/aigw/product/guardrails/pii-redaction)
