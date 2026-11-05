# 0014. Model gateway and providers

Status: Proposed
Date: 2026-11-04
Plan decisions: 4, 20

## Context

The server shouldn't hold provider keys, and Askbar should work with most cloud and self-hosted models.

## Decision

- **Keys and budgets:** the admin stores keys and sets budgets. The model gateway holds keys in memory and enforces budgets.
- **Fallback:** v1 has no automatic provider fallback. If a provider fails, the admin adds or selects another one.
- **Format:** providers default to the OpenAI-compatible format, using `/v1/models` and `/v1/chat/completions`.
- **Testing:** the admin's Test button also tests streaming.
- **Anthropic adapter:** keep the direct Anthropic support in `main`.

**Subscription login through `cpa`**

`cpa` is optional, preconfigured, and connected to the model gateway automatically. A deployment still works with direct API keys alone.

1. First start creates the `cpa` config and credentials and adds `cpa` to the model gateway.
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

A compromised server can still misuse model access through the gateway, within its budgets and configured providers ([0016](0016-risk-register.md)). The owner rechecks Anthropic's policy before the 1.0 release ([0002](0002-v1-scope-limits-and-ship-criteria.md)).
