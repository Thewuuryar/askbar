---
id: audit/alert-kinds
title: Alert kinds
summary: Every kind of alert Askbar can send, with its exact name, severity and the links that may send it.
keywords: [alerts, alert kinds, severity, alerter, auditor, admin, ntfy, email]
settings: [notifications]
---

# Alert kinds

Every alert has one kind from this fixed list, a severity and a few typed fields. The alerter turns the kind into its own sentence, so a sender cannot make an alert say anything else ([alerter](../../guide.md#the-alerter)). The names below are the names on the wire and in the audit record. The table is copied from `crates/askbar-proto/src/alerter.rs` (`Kind`, `severity()` and `senders()`); when they differ, the code is right and this page is the bug.

Who sends a kind is one of three links: `auditor` (the audit store's container), `admin` (this site) or `alerter` (the alerter itself, for releases and tests). The [glossary](../../architecture.md#glossary) defines the names.

| Kind | Severity | Sent by | When |
| --- | --- | --- | --- |
| `unsealed` | warning | auditor | Sealed audit content was opened. |
| `unseal_timeouts_changed` | warning | auditor | The unseal time limits were changed. |
| `retention_changed` | warning | auditor | Audit retention was changed, to the number of days given. |
| `backup_key_changed` | warning | auditor | The backup key was changed. |
| `audit_disk_low` | warning | auditor | Under 1 GiB is free on the audit store. |
| `audit_disk_critical` | critical | auditor | Under 256 MiB is free on the audit store. |
| `component_silent` | critical | auditor | Nothing has come from the server, `model-gateway`, the alerter or the admin for five minutes. |
| `caddy_silent` | warning | auditor | A reply was recorded, and fifteen minutes later Caddy has sent no access line. |
| `restart_loop` | warning | auditor, admin | The server, `model-gateway`, the alerter or the admin restarted three times within ten minutes. |
| `chain_mismatch` | critical | admin | The audit records no longer match what the admin stored: one alert for each finding. |
| `rejected_token` | warning | admin | A service refused the admin's token on a link: one for each episode. |
| `budget_threshold` | warning, then critical | admin | A day's or a month's token budget reached 80% (warning) or 100% (critical). |
| `backup_failed` | critical | admin | A scheduled backup could not be made. |
| `backup_missed` | critical | admin | No backup has finished for 26 hours. |
| `restore_performed` | warning | admin | A backup was restored into this install. |
| `release_available` | info | alerter | A newer Askbar release is out. |
| `test` | info | alerter | A test alert, sent from Notifications. |

## Notes

- The admin's bell does not list `chain_mismatch` or `rejected_token` as rows, because their banners already show them (`BELL_SKIP_KINDS` in `services/admin/src/ui/app.js`).
- The alerter limits repeats of the same alert and counts the repeats it holds back (`services/alerter/src/limits.rs`); see [The alerter](../../guide.md#the-alerter).
