---
id: admin-ui/page-layouts
title: Page layouts
summary: The layouts the admin's pages use: an index, list pages, sectioned settings pages and the reference article.
keywords: [layouts, index, list, settings, article, pages, templates]
settings: []
---

# Page layouts

Every admin page sits in one frame: the side menu, the status bar and the page header, with the page's sections in `main`. The frame is `services/admin/src/ui/templates/base.html`. Beyond the frame, the pages use these layouts. The components they are built from are on [Components](components.md).

## Index

The overview of a group of pages: one line for each item, with what it is set to now. The Settings page is the index of the settings pages (`settings.html`).

## List pages

A table or list of records, with a heading and a hint under the title. The audit pages are lists: Admin actions (`audit_admin.html`), Model calls (`audit_calls.html`), Sealed access (`audit_sealed.html`) and Routing (`audit_routing.html`).

## Sectioned settings pages

A settings page is one `section` per settings section, each with its name and a few lines of controls, and a save bar when a value changes. Providers, People, Services, Network, Security, Backup, Notifications and General use it (`providers.html`, `people.html`, `services.html`, `network.html`, `security.html`, `backup.html`, `notifications.html`, `general.html`). Each section's name is a help topic: see [the reference's index](../index.md#how-these-pages-are-written).

## Article

A page of reference text, read rather than used. The reference pages use it, and `reference.html` is its template. Until the reference is rendered in the admin (plan step 7.2) the Reference page only says so.

Other pages, Home and Alerts, do not fit these four. The full list of layouts is rule 5 of the redesign plan (`docs/plan-admin-redesign.md`).
