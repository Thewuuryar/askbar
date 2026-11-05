# 0030. Sealed audit content

Status: Accepted
Date: 2026-11-04
Source: required by v1 decisions

## Context

Audit records must support investigations, and reading their content must take an explicit, logged act by the owner. The auditor and a compromised admin can't read it at rest.

## Decision

1. **Sealing:** the server encrypts prompts, replies, and provider errors with a public key made from the audit passphrase, using Argon2id to derive an X25519 key.
2. **No private key:** the server and auditor never hold the passphrase or private key. The auditor stores only metadata and ciphertext. The server can still read its own space files, so sealing protects the audit copy, not the history.
3. **Metadata:** the admin can read metadata such as time, user, space, providers tried, step timings, tokens, and outcome.
4. **`model-gateway`:** its independent records contain metadata only.
5. **No plain copies:** prompts, replies, and provider errors appear only in sealed payloads, never in logs or error text.
6. **Missing key:** until an audit key exists, Askbar answers `503 audit_key_missing` instead of recording unsealed.
7. **Rotation:** a new passphrase seals new content with a new key. Older content still needs the old passphrase. Each sealed record carries a key ID, and audit metadata keeps a registry of key IDs with created and retired dates, so the admin can name the passphrase a record needs. Records aren't re-sealed in v1, and old content ages out under retention.
8. **Loss:** a lost passphrase can't be recovered, and content sealed under it is gone for good.

## Alternatives considered

- **Plaintext content behind an admin gate:** rejected, because the gate would be software only and a compromised admin could skip it. It would also store every conversation a second time in plaintext, including in backups.
- **A key held by the server or admin:** rejected, because compromising either would open all content.

## Consequences

A compromised server can't open records it already sealed, but it can still read conversations in its space files. Sealed content can't be searched by the server. Rotating after a suspected compromise protects new content only. Tests and `check-isolation.sh` confirm that no unsealed content is recorded.
