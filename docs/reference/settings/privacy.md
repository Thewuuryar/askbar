---
id: settings/privacy
title: Privacy
summary: Finding personal data and secrets in what is sent to model providers and the web, what to do with each kind, and the terms to always or never redact.
keywords: [privacy, redaction, PII, personal data, placeholder, pseudonymize, block, allow, terms, detect only, enforce]
settings: [privacy]
---

# Privacy

Looks for personal data and secrets in the messages sent to a model provider and in the search queries and page addresses sent to the web, and can replace or refuse them. Detection is a filter, not a guarantee: read what it does not do below before you rely on it.

## Redaction

The switch turns the check on or off for everyone; it is off until you turn it on. The mode says what it does with what it finds. *Detect only* counts and changes nothing: use it first, and look at the counts on the `redact` step in [Model calls](../audit/model-calls.md) to see what it would have done. *Enforce* replaces values with placeholders and refuses the messages that hold what you set to block. A change reaches the model gateway and the MCP front within a moment of saving.

## Types

One row for each kind of value it knows: email address, phone number, payment card number, IBAN, national ID number, IP address, API key, GitHub token, JSON Web Token and private key. Each has an action and shows its default beside it. *Replace with a placeholder* (pseudonymize) swaps the value for a placeholder such as `EMAIL_1` before the provider sees it and puts the value back in the reply. *Block the message* refuses the message with a notice that names the type, never the value. *Allow* lets the type through. Choosing the default again removes your change. The actions apply in enforce mode only.

## Terms

Two lists, one term per line, up to 500 terms of up to 100 characters. *Always redact* terms (a project name, a customer) are replaced by a placeholder wherever they appear, as `CUSTOM`. *Never redact* terms (an address of your own, a name that looks like a value) are left alone even when a type matches. Spaces around a term and empty or repeated lines are dropped when you save. The terms are personal data: they are kept only in this admin's database and sent to the gateway and the MCP front; they are never written to a log, an alert, an error or the audit record of the change, which holds only the switch, the mode and the counts.

## What it does not do

- It finds structured data: email addresses, phone numbers, card and ID numbers, IBANs, IP addresses, keys and tokens. It never finds names, addresses or indirect identifiers, and it can miss a value written in an unusual way.
- A placeholder can hurt a question about the value itself (for example, asking what is wrong with an address). Put such a value under *Never redact*, or set its type to allow.
- It applies to messages sent to providers by the model gateway (replaced or blocked) and to search queries and fetch addresses by the MCP front (keys are refused, other private data is left out). Nothing else is checked: not the replies shown to people, and not what the server stores.
- Only counts, by type, are recorded, never values.
