---
id: audit/model-calls
title: Model calls
summary: What the server recorded about each reply, the figures for a range, and the trace of a single reply.
keywords: [model calls, audit, trace, replies, tokens, latency, errors, status, provider, sealed]
settings: [audit/calls]
---

# Model calls

What the server recorded about each reply: when, for whom, through which provider, how long it took and how it ended. The strip at the top shows the replies, errors, tokens and latency (median and 95th percentile) for the range you choose. Below it, the list can be filtered by range, status and provider, newest first; choosing a time opens the trace of that reply in the side panel, with a timeline of its steps and its details. The list holds metadata only: message content stays sealed, and opening it asks for the audit passphrase and is recorded (see [Sealed access](sealed-access.md)). Filtering by user or model, searching and exporting are shown greyed because the audit log cannot do them yet. How the records are kept is explained in [The Audit page](../../guide.md#the-audit-page).
