---
id: audit/routing
title: Routing
summary: The tries each request took, why a try failed, and how each provider did in a range.
keywords: [routing, tries, retries, failed tries, provider, credentials, audit]
settings: [audit/routing]
---

# Routing

The tries each request took, why a try failed, and which providers failed. Pick a request on [Model calls](model-calls.md) to see its tries here.

## This request

The tries of the request you picked on Model calls: for each, the provider, the try number, its status, the kind of failure, the reason and the latency. Until a request is picked, the section says so.

## Providers, in the chosen range

One row for each provider in the range chosen beside the heading: its tries, retries, failed tries and credentials. A range with no recorded tries says so.
