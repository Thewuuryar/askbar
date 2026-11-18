---
id: home
title: Home
summary: How Askbar is doing and what needs you: what needs attention, the request numbers and chart, the services, the backup and the clients.
keywords: [home, overview, needs attention, requests, errors, tokens, cost, latency, chart, services, backup, clients]
settings: [home]
---

# Home

How Askbar is doing, and what needs you. The page opens with what needs attention, then the request numbers and a chart for the range you choose, then the services, the backup and the clients. The range selector at the top of the page sets how far back the numbers and the chart look.

## Needs attention

The critical and warning alerts, then anything still to set up, each with a link to where it is fixed: no provider, no approved model, no default model, no user, no paired client, no backup key, a backup destination that cannot be written to, no backup made yet, a restore drill that is due, and a missing host name while HTTPS is on. Info alerts are not listed here; [Alerts](alerts.md) lists every alert, and the bell in the status bar counts the same critical and warning ones.

## Request numbers

Requests, error rate, tokens, cost and the 95th percentile latency for the chosen range, read from the audit records of model calls. A figure that could not be read shows a dash, never 0, and a cost that could not be priced shows "unknown". The family budget and the latency of each provider are shown greyed because the server does not provide them yet.

## Requests per day

One bar for each day of the chosen range, with a one-line summary above it. The same figures are in a table for screen readers.

## Spend by person

Not available yet, so the section is greyed: the server does not track usage or caps for each person.

## Models used

Not available yet, so the section is greyed: the audit statistics are grouped by provider, not by model. The Model column of [Model calls](audit/model-calls.md) shows the model of each reply.

## Services

Whether each part of Askbar is running, with a link to the Services page, where each can be restarted: [Services](settings/services.md). How long each has run is greyed because the server does not report it.

## Backup

Whether backups are on schedule, overdue or failed on the last try, when the last one was made and when the next is due. Without a backup key it says so. [Backup & restore](settings/backup.md) has the controls.

## Clients

How many users and how many paired clients there are. [People & clients](settings/people.md) is where they are added and removed.
