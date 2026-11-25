---
id: settings/egress
title: Egress
summary: Where Askbar may connect, what the egress proxy refused, and your approvals for private addresses.
keywords: [egress, proxy, allowlist, approvals, blocked, outbound]
settings: [egress]
---

# Egress

Everything Askbar sends to the internet goes through the egress proxy, which refuses a destination the admin has not given it. Restart the proxy from [Services](services.md).

## Status

Whether the proxy has its allowlists, their version, how many tunnels are open, and how many requests it allowed and denied for each source since it started.

## Allowlist

What each source may reach, and where each entry comes from: a provider URL, the alert settings, `CPA_EGRESS_HOSTS`, `SEARXNG_EGRESS_HOSTS`, the fetch rule or an approval. The admin works the entries out; none is typed here.

## Approvals

A private destination (a server on your own network) is allowed only by an approval for one source: its host, port, whether plain HTTP is allowed, and a note. Adding or removing one is a recorded admin action.

## Blocked outbound requests

The 50 most recent refusals, newest first: when, which part asked (the source), the host, the port and why. It shows no URL path or content of the request. The list is what the proxy reported at its last check, every 12 seconds, and the proxy holds it in memory only: restarting the proxy empties it. If the proxy does not answer, the section says so instead of showing an empty list.
