---
id: index
title: Reference
summary: What each admin setting does, how the admin site is built, and what the audit alerts mean. Start here.
keywords: [reference, help, settings, admin, guide, outline]
settings: []
---

# Reference

This is the reference for the admin site: one page for each page of the admin (Home, Alerts, the Audit pages and the settings pages), and pages for the admin's building blocks and the alert kinds. The admin shows these pages in its Reference section; this copy is the same text on GitHub.

## Overview

- [Home](home.md): how Askbar is doing and what needs you.
- [Alerts](alerts.md): everything that needs your attention, by severity.

## Admin settings

- [Providers & models](settings/providers.md)
- [People & clients](settings/people.md)
- [MCP](settings/mcp.md)
- [Egress](settings/egress.md)
- [Search](settings/search.md)
- [Services](settings/services.md)
- [Network](settings/network.md)
- [Security](settings/security.md)
- [Privacy](settings/privacy.md)
- [Backup & restore](settings/backup.md)
- [Notifications](settings/notifications.md)
- [General](settings/general.md)

## Admin UI

- [Components](admin-ui/components.md): the shared building blocks the pages are made from.
- [Page layouts](admin-ui/page-layouts.md): the layouts the pages use.

## Audit and alerts

- [Model calls](audit/model-calls.md): what the server recorded about each reply.
- [Admin actions](audit/admin-actions.md): what was changed in the admin.
- [Sealed access](audit/sealed-access.md): opening sealed content, and who did.
- [Routing](audit/routing.md): the tries each request took.
- [Alert kinds](audit/alert-kinds.md): every kind of alert, its severity and who sends it.

## Where the rest is

- [The guide](../guide.md) covers installing, running and backing up Askbar.
- [Architecture](../architecture.md) covers how the parts fit, with the [glossary](../architecture.md#glossary) of names.
- [Decisions](../adr/README.md) (ADRs) record why things are the way they are.

## How these pages are written

Each page starts with front matter: `id` (the page's path without `.md`, unique), `title`, `summary`, `keywords`, and `settings` (the admin pages it explains, by their address under `/admin/`, or `[]`). Each settings section is a heading on its page. A help topic is a page id and a heading, written `settings/network#host-name`; the list of topics the admin's ? buttons use is `HELP_TOPICS` in `services/admin/src/constants.rs`. `scripts/check-reference.sh` checks the ids, the links and the topics; `scripts/check-docs.sh` runs it.
