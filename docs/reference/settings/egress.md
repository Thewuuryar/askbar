---
id: settings/egress
title: Egress
summary: Where Askbar may connect, what the egress proxy refused, and your approvals for private addresses.
keywords: [egress, proxy, allowlist, approvals, blocked, outbound, restart]
settings: [egress]
---

# Egress

Everything Askbar sends to the internet goes through the egress proxy, which refuses a destination the admin has not given it. This page shows what it allows, what it has refused and the approvals you give for private addresses. The proxy is one of the services on [Services](services.md).

## Status

Whether the proxy has its allowlists (until the admin first pushes them it refuses everything), their version, how many tunnels are open, and how many requests it allowed and denied for each source since it started.

## Allowlist

Read only. Each source (`model-gateway`, `alerter`, `cpa`, `searxng` and `fetch`) with its entries, and where each entry comes from: a provider URL, the alert settings, `CPA_EGRESS_HOSTS`, `SEARXNG_EGRESS_HOSTS`, the fetch rule or an approval. The admin works the entries out from its settings and these environment variables; none is typed here. The `fetch` rule lets a page be read from any public name on ports 80 and 443, never from an address literal or a private address. The origin label is for this page only; the proxy does not receive it.

## Approvals

A private destination (a server on your own network) is allowed only by an approval for one source, which is one of `model-gateway`, `alerter` or `cpa`. An approval has a host, a port, whether plain HTTP is allowed and a note of up to 200 characters. Add one with its fields, or remove one from the list. Each add or remove is an audited admin action, and a validation error is shown as text on the page. The admin then pushes the new allowlist to the proxy.

## Blocked outbound requests

The 50 most recent refusals, newest first: when, which part asked (the source), the host, the port and why. It shows no URL path or content of the request. The list is what the proxy reported at its last check, every 12 seconds. The proxy holds it in memory only, so restarting the proxy empties it. If the proxy does not answer, the section says so instead of showing an empty list.

## Restarting the proxy

The Services page has an Egress proxy row with its own Restart button, behind the same confirmation as the other services. The admin asks the proxy to restart, waits for a new process to answer, and then pushes its allowlists again, because the proxy holds them in memory. If it has not come back within a minute, the admin says so and you can check it on the host.
