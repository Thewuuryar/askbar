# 0030. Sealed audit content

Status: Proposed
Date: 2026-11-04
Source: required by v1 decisions

## Context

Audit records must support investigations without letting the server or auditor read private text.

## Decision

1. **Sealing:** the server encrypts prompts, replies, and provider errors with a public key made from the audit passphrase, using Argon2id to derive an X25519 key.
2. **No private key:** the server and auditor never hold the passphrase or private key. The auditor stores only metadata and ciphertext.
3. **Metadata:** the admin can read metadata such as time, user, space, providers tried, step timings, tokens, and outcome.
4. **`model-gateway`:** its independent records contain metadata only.
5. **No plain copies:** prompts, replies, and provider errors appear only in sealed payloads, never in logs or error text.
6. **Missing key:** until an audit key exists, Askbar answers `503 audit_key_missing` instead of recording unsealed.
7. **Rotation:** a new passphrase seals new content with a new key. Older content still needs the old passphrase.
8. **Loss:** a lost passphrase can't be recovered, and content sealed under it is gone for good.

## Alternatives considered

Not recorded.

## Consequences

A compromised server can't open records it already sealed, but it can still read conversations in its space files. Sealed content can't be searched by the server. Rotating after a suspected compromise protects new content only. Tests and `check-isolation.sh` confirm that no unsealed content is recorded.
