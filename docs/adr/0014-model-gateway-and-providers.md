# 0014. `model-gateway` and providers

Status: Accepted
Date: 2026-11-04
Plan decisions: 4, 20

## Context

The server shouldn't hold provider keys, and Askbar should work with most cloud and self-hosted models.

## Decision

- **Keys and budgets:** the admin stores keys and sets budgets. `model-gateway` holds keys in memory and enforces budgets. The admin pushes budgets to `model-gateway` and to the server's control database ([0009](0009-spaces-and-storage.md)). The `model-gateway` copy is authoritative, and the server's copy only lets it refuse early and show usage.
- **Fallback:** v1 has no automatic provider fallback. If a provider fails, the admin adds or selects another one.
- **Format:** providers default to the OpenAI-compatible format, using `/v1/models` and `/v1/chat/completions`.
- **Model catalog:** the admin picks approved models from each provider's model list and sets one default. IDs are provider-qualified (`provider/model`). The admin pushes the catalog to `model-gateway`, which serves it to the server, and the server offers it to clients to choose from for each chat. A conversation stores its model. `model-gateway` refuses any model outside the catalog, and the server checks too. The admin can't save a catalog without a default.
- **Testing:** the admin's Test button also tests streaming, and shows pass or fail and time to first token. An optional details view shows the request path and method, status code, error type, and timings, with secrets redacted, and never a response or error body. `model-gateway` runs the test with a fixed prompt, the details are never stored, and the audit record holds metadata only.
- **Anthropic adapter:** keep the direct Anthropic support in `main`.

**Subscription login through `cpa`**

`cpa` is optional, preconfigured, and connected to `model-gateway` automatically. A deployment still works with direct API keys alone. `cpa` keeps its subscription credentials on its own volume, an exception to [0028](0028-service-boundaries-and-config-delivery.md), and [0013](0013-backups.md) backs them up.

1. First start creates the `cpa` config and credentials and adds `cpa` to `model-gateway`.
2. The admin selects a provider and opens the authentication link.
3. If the provider redirects back to the server, the login finishes automatically. Otherwise, the admin pastes the returned URL, and the admin service extracts the code.
4. A pasted URL must match the expected address and state, expires after 10 minutes, works once, and is never logged.
5. Both providers are tested against the pinned `cpa` image before v1 ships.

| Provider | V1 status |
| --- | --- |
| Anthropic | Uses official flow pre release, may be disabled in v1 release |
| OpenAI | Uses official flow pre release, may be disabled in v1 release |

## Alternatives considered

- **Automatic provider fallback:** rejected for v1.

## Consequences

A compromised server can still misuse model access through `model-gateway`, within its budgets, configured providers, and approved models ([0016](0016-risk-register.md)).
