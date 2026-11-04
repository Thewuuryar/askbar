# 0004. Audit content is sealed

Status: Accepted (where the store lives is changed by [0009](0009-the-auditor.md), which is Proposed)
Date: 2026-11-03 (backfilled)

## Context

The goals document asks to "log everything agents do, so you can investigate later", and that only the admin can read it. Replies carry private text.

## Decision

- The server records a trace of every reply: when, for whom, providers tried, step timings, tokens, outcome. The metadata is readable in the admin's Audit tab.
- The content (prompts, replies, provider errors) is encrypted by the server with a public key made from the audit passphrase (Argon2id to an X25519 key). The server never holds the passphrase or the private key and cannot read what it wrote.
- Reading content happens in the admin, with the passphrase, one item at a time. Each opening is written to the access history (request, key, time; never the content).
- Retention: default 30 days, minimum 7. Until an audit key exists the server answers `503 audit_key_missing` instead of recording unsealed.
- The records are kept by the auditor, not the server ([0009](0009-the-auditor.md)).
- The passphrase cannot be recovered. Rotating it seals new content with the new key; old content needs the old passphrase.

User-facing description: README, "The Audit tab".

## Alternatives considered

Not recorded.

## Consequences

A compromised server cannot open sealed audit content. It can still read conversations, which it keeps in its own database, so sealing does not hide conversations from the server. A lost passphrase loses sealed content for good. Search over content is not possible on the server. Enforced by `askbar-server/tests/audit.rs` and the isolation script.
