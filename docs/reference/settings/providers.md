---
id: settings/providers
title: Providers & models
summary: Where answers come from (providers and their accounts) and which models people can pick.
keywords: [providers, models, accounts, Claude, ChatGPT, catalog, CLIProxyAPI]
settings: [providers]
---

# Providers & models

Where answers come from, and which models people can pick. A provider is an upstream source of model output; this page holds the providers you have set up and the list of models that people can use, which you choose from the models your providers offer. The accounts section covers the Claude and ChatGPT accounts that CLIProxyAPI signs in to. How CLIProxyAPI runs is in the [guide](../../guide.md#cliproxyapi); the meaning of *provider* is in the [glossary](../../architecture.md#glossary).

## Providers

The services Askbar gets answers from. Add a provider, check that it works, and set what it costs per million tokens. Leave the cost empty if you do not know it: cost then shows as unknown, not as zero.

## Claude and ChatGPT accounts

Use a Claude or ChatGPT subscription as a provider. The CPA provider answers once an account is connected.

## Models people can use

The list of models people can pick. It applies to every user and client, and a message that names no model uses the default.
