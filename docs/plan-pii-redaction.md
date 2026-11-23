# Plan: PII detection and redaction in model-gateway

Status: proposed, after 1.0. Track 2 in [plan-priorities.md](plan-priorities.md): it starts after web search. Nothing here is decided until the owner accepts the ADR work in Phase 0. Rules for agents: [AGENTS.md](../AGENTS.md).

Related plans: [context checks and canaries](plan-canaries.md), which adds to the reply stream filter this plan builds, and [audit record viewing](plan-audit-viewer.md), which shows what this plan records.

## Summary

`model-gateway` is the last hop before a provider, so it detects personal data in each request and replaces it with a placeholder such as `⟦PERSON_a3f91c⟧`. As the reply streams back, the gateway swaps placeholders back to the real values, so the user never sees them. Providers see placeholders; the user's history keeps what they typed.

Detection has two tiers:

1. **Tier 1, always on, in process:** patterns with checksums for structured data (email, phone, card numbers with Luhn, IBAN with mod-97, national ID numbers, IP addresses) and secrets people paste (provider API keys, GitHub tokens, JWTs, private keys). Pure Rust, well under a millisecond.
2. **Tier 2, optional, a separate container:** a small named-entity model on CPU for names, addresses and organisations that patterns miss. It holds no secrets and has no network beyond its link to the gateway.

Detection is a filter, not a guarantee. On hard real-world text the best small models score an F1 of about 0.4–0.5, far below the 0.8–0.96 their authors report on synthetic data. Indirect identifiers ("my son's teacher at the school on Elm Street") pass through. The admin documentation must say so.

## Decisions

| Area | Decision |
| --- | --- |
| Where | `model-gateway`, after the gates and before the attempt loop; one redaction per turn, reused for retries |
| Placeholder | `⟦TYPE_xxxxxx⟧`, where `xxxxxx` is the first 6 hex characters of HMAC-SHA256(gateway secret, conversation id, type, normalised value) |
| Consistency | The same value gets the same placeholder in every turn of a conversation, so the model sees one consistent person and the cached prompt prefix stays identical. Different conversations get different placeholders |
| Gateway secret | Random at gateway start, in memory only. A restart changes placeholders and costs one cache miss: every request re-redacts the whole history |
| Reverse map | Built for each request from what that request contained. Nothing is stored |
| Restore | A reply stream filter, built here and extended by [canaries](plan-canaries.md), holds back text from `⟦` until `⟧` or 24 characters, then substitutes or passes it through. It also accepts the placeholder without brackets |
| Actions | `allow`, `pseudonymize` (restored in the reply), `mask` (not restored), `block` (turn refused at a gate), `detect_only` |
| Default actions | `block`: secrets, private keys, card numbers. `pseudonymize`: email, phone, IBAN, national ID, address, person. `detect_only`: date, URL, IP. `allow`: organisation |
| Code blocks | Inside fenced code, secrets only by default: identifiers, hashes and UUIDs are the main false positives |
| Reply scanning | Separate from restore. Default `detect_only`; `redact_lines` holds back until each newline |
| Tier 2 down | `refuse` (default, fail closed) or `regex_only`; recorded either way |
| Tier 2 model | `knowledgator/gliner-pii-base-v1.0`, UINT8 ONNX (Apache-2.0, about 200 MB). Not Piiranha: its licence (CC-BY-NC-ND) forbids modification |
| Audit | The server still seals the request as typed. The gateway also seals `provider_request`, what the provider actually received, on the attempt span (the auditor already lets the gateway append payloads). Unsealed `attrs.pii` holds counts by type and actions, never values |
| Terms lists | Always-redact and never-redact terms are themselves personal data: stored in `admin.db`, never in alerts or audit attributes |

## Config

Added to `GatewayConfig` in `crates/askbar-proto/src/gateway.rs` as `#[serde(default)]`, pushed through `PUT /config`.

```rust
pub struct PiiConfig {
    pub enabled: bool,                               // default true (tier 1)
    pub detector: DetectorConfig,                    // { enabled: false, on_unavailable: Refuse, timeout_ms: 400 }
    pub entities: BTreeMap<String, EntityRule>,      // type -> { action, min_score }
    pub custom_patterns: Vec<CustomPattern>,         // { name, regex, score, action }
    pub allow_terms: Vec<String>,
    pub deny_terms: Vec<String>,                     // always pseudonymized as CUSTOM
    pub code_blocks: CodeBlockMode,                  // secrets_only | full | skip
    pub reply_scan: ReplyScan,                       // off | detect_only | redact_lines
    pub user_overrides: BTreeMap<String, PiiOverride>, // user id -> partial rules
}
```

`check_config` compiles every pattern with the `regex` crate's linear-time engine and a size limit, and rejects the whole config if one fails, so the gateway keeps its previous config.

## Admin UI: a new Privacy page

1. **Detection:** tier 1 on or off; tier 2 on or off, its model, what happens when it is unavailable, and its measured p50 and p95 latency.
2. **Entity table:** one row per type with an action, a minimum score and "also scan replies". Presets: Strict, Balanced (default), Off.
3. **Terms:** always-redact and never-redact lists, one term per line.
4. **Custom patterns:** name, pattern, score and action, validated when saved.
5. **Per-user overrides** on each user's page, for example to let a user's own name through.
6. **Try it:** a text box that posts to a new gateway admin route, `POST /pii/test`, and shows the spans found, types, scores, placeholders and timing. The text is not stored; the test is recorded as an admin action without it.
7. **Counts** for the last 7 days by type and action, from audit attributes.

## Alerts

New `Kind` values: `PiiBlocked` (info, at most one per user per hour), `PiiDetectorDown` (warning), `SecretInPrompt` (warning: a pasted key should probably be rotated).

## Owner gates

- **Phase 0:** accept the ADR work.
  - Tier 1 and the placeholder scheme amend [ADR 0054](adr/0054-model-gateway.md) and [ADR 0014](adr/0014-model-gateway-and-providers.md).
  - The new `provider_request` payload amends [ADR 0011](adr/0011-sealed-content-access.md) and [ADR 0051](adr/0051-auditor.md).
  - Tier 2 is a new container, so it needs a new component ADR, its name checked under [ADR 0038](adr/0038-names-and-new-components-without-a-name-check.md) (proposed: `pii-detector`), and the hardening in [ADR 0032](adr/0032-container-hardening.md). It is ADR 0058: 0056 is reserved for [tiered routing](plan-tiered-routing.md) and 0057 for the MCP gateway.
  - The `ort` dependency and model weights need the licence check in [ADR 0026](adr/0026-dependency-license-check.md).
- **Before Phase 2:** two weeks in `detect_only`, then the owner reviews false positives with Try it.
- **Before Phase 4:** the owner annotates 300–500 spans from real use and the detector is benchmarked on the actual host.

## Phases

| Phase | Content | Status |
| --- | --- | --- |
| 0 | ADR amendments and the tier 2 component ADR | not started |
| 1 | Tier 1 crate, `detect_only` everywhere | not started |
| 2 | Pseudonymize, restore, block, `provider_request` payload | not started |
| 3 | Admin Privacy page | not started |
| 4 | Tier 2 detector container | not started |
| 5 | Reply scanning; surrogates if answers suffer | not started |

### Phase 0: records

- [ ] **PII-P0.1** `docs(adr): amend 0054 and 0014: PII detection and placeholders` · sonnet
  Accept: the Decisions table is recorded as rules; the limits paragraph from the Summary is included.
- [ ] **PII-P0.2** `docs(adr): amend 0011 and 0051: provider request payload and PII attributes` · haiku · after PII-P0.1
  Files: the two ADRs, `docs/reference/audit/model-calls.md`.
- [ ] **PII-P0.3** `docs(adr): propose the PII detector component` · sonnet · after PII-P0.1
  Do: a component ADR from `docs/adr/0000-template.md`: no network but the gateway link, read-only weights volume, no secrets, per-link token from `bootstrap`, never talks to the auditor.
  Accept: `scripts/check-docs.sh` passes with the new row in `docs/adr/README.md`.

### Phase 1: tier 1, detect only

- [ ] **PII-P1.1** `feat(proto): add PII settings` · haiku
  Files: `crates/askbar-proto/src/gateway.rs`. Accept: defaults as above; old config parses.
- [ ] **PII-P1.2** `feat(pii): add the askbar-pii crate` · sonnet · after PII-P1.1
  Files: `crates/askbar-pii/` (new), `Cargo.toml`.
  Do: `detect(text, &Rules) -> Vec<Span>` with types, scores and byte ranges; validators for Luhn and IBAN; secret rules; code block handling; allow and deny terms; merge overlaps, preferring validated spans.
  Accept: table tests per type, including false-positive cases (UUIDs, git hashes, version numbers, the names Will, Rose and Mark); a benchmark shows under 1 ms for 8 KB.
- [ ] **PII-P1.3** `feat(model-gateway): detect PII and record counts` · sonnet · after PII-P1.2
  Files: `services/model-gateway/src/{lib,pii}.rs`, tests.
  Do: run detection on the system prompt and messages after `gates()`; cache results by message hash in a small in-memory LRU so each turn scans only new text; set `attrs.pii = {version, counts, actions, detector}`; send nothing different to the provider yet.
  Accept: counts appear on the attempt span; a test greps logs and attributes for a planted email and finds nothing.

### Phase 2: pseudonymize and restore

- [ ] **PII-P2.1** `feat(model-gateway): replace PII with placeholders` · sonnet
  Do: build placeholders as in Decisions; apply each type's action; add one short line to the system message saying placeholders stand for private values and should be reused as written.
  Accept: the same value gives the same placeholder across two turns of one conversation and a different one in another conversation.
- [ ] **PII-P2.2** `feat(model-gateway): add the reply stream filter and restore placeholders` · sonnet · after PII-P2.1
  Files: `services/model-gateway/src/{lib,filter,pii}.rs`, tests.
  Do: wrap the delta callback in `run_attempts` with a `StreamFilter` chain (`push(&str) -> String`, `finish() -> String`) that holds back text that could start a marker; the canary plan adds its filters to this chain (C3.2). Add the PII restore filter using the per-request reverse map; record `restored` in attributes.
  Accept: property tests split placeholders at every byte; unknown placeholders pass through unchanged.
- [ ] **PII-P2.3** `feat(model-gateway): refuse a turn with blocked PII` · haiku · after PII-P2.1
  Do: a gate refusal with error code `pii_blocked`, naming the type only; the server returns it as `application/problem+json` ([ADR 0023](adr/0023-api-error-format.md)).
  Accept: a pasted `sk-ant-` key is refused, and the refusal and the alert carry no part of it.
- [ ] **PII-P2.4** `feat(model-gateway): seal what the provider received` · haiku · after PII-P2.1
  Do: seal `provider_request` on the attempt span when redaction changed anything.
  Accept: the payload decrypts in the sealed view to the redacted text.
- [ ] **PII-P2.6** `feat(mcp-gateway): check search queries and fetch URLs for PII` · sonnet · after PII-P1.2
  Files: the MCP gateway front's search and fetch handlers, tests.
  Do: run the `askbar-pii` tier 1 detector on each search query and fetch URL before the audit span. Secrets refuse the call; other PII is removed from the query, the call goes ahead, and the reply says a term was left out. Record counts on the tool span, never values. Placeholders are no use to a search engine, so queries are never pseudonymized.
  Accept: a query containing an email address reaches the fixture search engine without it; a query with a provider key is refused.
- [ ] **PII-P2.5** `feat(auditor): PII alerts` · haiku · after PII-P2.3
  Files: `crates/askbar-proto/src/alerter.rs`, `services/auditor/src/rules.rs`.

### Phase 3: admin

- [ ] **PII-P3.1** `feat(admin): store and push PII settings` · sonnet
  Files: `services/admin/src/{db,gateway_link,admin_api}.rs`, `docs/openapi/admin.v1.yaml`.
  Do: settings JSON and terms in `admin.db` (append a migration); per-user overrides on users; validation errors name the failing pattern.
- [ ] **PII-P3.2** `feat(model-gateway): PII test route for the admin` · haiku · after PII-P3.1
  Files: `services/model-gateway/src/lib.rs`, `docs/openapi/model-gateway.v1.yaml`.
  Accept: admin listener only; never logs the text.
- [ ] **PII-P3.3** `feat(ui): Privacy page` · sonnet · after PII-P3.2
  Files: `services/admin/src/pages.rs`, a new template, `app.js`, admin browser test.
  Accept: presets, the entity table, terms, patterns, Try it and counts work; text is written with `textContent` only.

### Phase 4: tier 2 detector

- [ ] **PII-P4.1** `feat(pii-detector): add the detector service` · sonnet · after PII-P0.3 accepted
  Do: axum and `ort` (or `gline-rs`); inference in `spawn_blocking`; a warm-up call every few minutes, because the first call after idle is reported 2–3 times slower; `GET /status` with p50 and p95.
- [ ] **PII-P4.2** `feat(compose): add the detector container` · sonnet · after PII-P4.1
  Do: its own network shared with `model-gateway` only; read-only root and weights; no secrets.
  Accept: `scripts/check-isolation.sh` passes and shows the detector can reach nothing else.
- [ ] **PII-P4.3** `feat(model-gateway): use the detector` · sonnet · after PII-P4.2
  Do: call it for text not already scanned; merge spans; apply `on_unavailable`; raise `PiiDetectorDown`.
  Accept: with the detector stopped, the default refuses and `regex_only` continues, and both are recorded.

### Phase 5: replies and surrogates

- [ ] **PII-P5.1** `feat(model-gateway): scan replies` · sonnet
  Do: `detect_only` counts in attributes; `redact_lines` through the stream filter.
- [ ] **PII-P5.2** Surrogates, only if the owner finds answers suffer: realistic fake values instead of placeholders. One 2026 study measured better query similarity than bracket placeholders, but restoring becomes fuzzy, so it is opt-in for each type.

## Risks

- **Latency:** tier 2 is reported at 150–250 ms per call on 4 CPU cores, against a target of 200 ms at p95 before the first token. The cache keeps it to the newest message; the admin shows measured latency.
- **Answer quality:** placeholders hurt questions about the value itself ("is this address valid?"). Per-user overrides and `allow` cover it.
- **Split identifiers:** a name in one message and an address in another are each judged alone.
- **Memory:** the detector needs 200–800 MB; check the host before enabling it.

## Sources

- [OpenAI Privacy Filter](https://openai.com/index/introducing-openai-privacy-filter/) · [model card](https://huggingface.co/openai/privacy-filter)
- [knowledgator/gliner-pii-base-v1.0](https://huggingface.co/knowledgator/gliner-pii-base-v1.0) · [urchade/gliner_multi_pii-v1](https://huggingface.co/urchade/gliner_multi_pii-v1)
- [GLiNER2-PII and the SPY benchmark](https://arxiv.org/pdf/2605.09973)
- [gline-rs](https://docs.rs/crate/gline-rs/latest) · [Rust GLiNER2 production write-up](https://users.rust-lang.org/t/building-a-production-multilingual-pii-detection-system-in-rust-with-gliner2/139964)
- [Piiranha licence](https://huggingface.co/iiiorg/piiranha-v1-detect-personal-information)
- [SurrogateShield](https://arxiv.org/pdf/2606.29567) · [Anonymous-by-Construction](https://arxiv.org/html/2603.17217)
- [LiteLLM PII masking](https://docs.litellm.ai/docs/proxy/guardrails/pii_masking_v2) · [Kong AI Sanitizer](https://developer.konghq.com/plugins/ai-sanitizer/) · [Portkey PII redaction](https://portkey.ai/docs/aigw/product/guardrails/pii-redaction)
