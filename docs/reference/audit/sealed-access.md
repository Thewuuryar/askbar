---
id: audit/sealed-access
title: Sealed access
summary: Opening sealed prompts and replies with the audit passphrase, and the list of who opened them.
keywords: [sealed access, unseal, seal, audit passphrase, sealed content, access history]
settings: [audit/sealed]
---

# Sealed access

Sealed prompts and replies stay closed until the audit passphrase opens them. Every opening is recorded. The passphrase is chosen during setup and cannot be recovered, and an admin API key cannot open sealed content.

## Session

Enter the audit passphrase to open sealed content for a while. It then stays open without more prompts, and seals itself after the idle or total time set under Security, when you press Seal, or when you sign out. You must have signed in shortly before; the page says how long. Opening, and each item you view, is recorded. The time limits are set in [Sealed content and audit records](../settings/security.md#sealed-content-and-audit-records).

## Who opened sealed content

Newest first. Each opening and each wrong passphrase is listed, with when it happened, the span, the key and the result; the content itself is never shown here. When a session ended and how it ended are shown greyed, because the access history records each attempt, not the end of a session.
