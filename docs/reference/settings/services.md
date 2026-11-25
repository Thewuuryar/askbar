---
id: settings/services
title: Services
summary: Whether each part of Askbar answers, and restarting one of them.
keywords: [services, restart, health, server, alerter, Caddy, CLIProxyAPI]
settings: [services]
---

# Services

Restart a part of Askbar. This page shows whether the server, `model-gateway`, the alerter, the admin, Caddy and CLIProxyAPI answer, and lets a signed-in person restart each one. The guide explains what each restart interrupts: [Starting and stopping services](../../guide.md#starting-and-stopping-services).

## Running services

Restart a part of Askbar. The list shows each service and whether it is running.

## Blocked outbound requests

Everything Askbar sends to the internet goes through the egress proxy, which refuses a destination the admin has not given it. This list shows the 50 most recent refusals, newest first: when, which part asked (the source), the host, the port and why. It shows no URL path or content of the request.

The list is what the proxy reported at its last check, every 12 seconds, and the proxy holds it in memory only: restarting the proxy empties it. If the proxy does not answer, the section says so instead of showing an empty list. A private address (a server on your own network) is allowed only by an approval for that source, which is set through the admin API route `/api/admin/v1/egress/approvals`. A refused request to the search engines has the source `searxng`, and one to a page has the source `fetch`. The built-in MCP tools each user holds (off by default) are set through the API in [People & clients](people.md#search-and-fetch-permissions).
