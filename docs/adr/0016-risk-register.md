# 0016. Risk register

Status: Accepted
Date: 2026-11-04
Plan decisions: 16
Kind: register

## Context

The owner's priority order is a compromised server, a compromised admin, then model breakout. V1 needs a written list of what it defends and what the owner accepts.

## Decision

The owner accepted the third list on November 4, 2026, and amended it on November 5, 2026. The first list describes the v1 target, so each item counts only once it's built and tested.

**Mitigated by design**

| Risk | Control |
| --- | --- |
| Compromised server steals provider keys or admin data | The server stores no keys and can't read admin files |
| Compromised server sends data out directly | The server has no outbound route |
| Compromised server rewrites audit history | The auditor accepts appends only |
| Admin deletes recent audit records | Auditor credentials can't edit, and retention can't go below 7 days |
| Service uses Docker to reach other volumes or the host | No service has the Docker socket |
| One service reads another's files | Separate volumes and user IDs |
| One service calls a service it shouldn't | Separate networks and per-link tokens |
| Query bug leaks another user's data | One store per space |
| Model changes settings or runs tools | Conversation only, no tools through 1.0 |

**Defend in v1**

| Risk | Required control | Test |
| --- | --- | --- |
| Admin or pairing brute force | No passkeys in v1; allowlist, rate limits, attempt cap, identical errors | Limit and lockout tests |
| Compromised admin, which holds live secrets | Fresh sign-in (within 10 minutes) to set or rotate the backup key, restore, delete a user, lower retention, create an API key and change the password, which an API key cannot do; independent audit; alerts, each sent by the component named in [0055](0055-alerter.md) | Admin can't alter records; alerts fire |
| Unknown changes to the audit log | Hash chain, with the admin checking the latest chain head against the auditor ([0010](0010-auditor.md)) | Tamper test |
| False or missing records after a server compromise | `model-gateway` records each call and refuses the call when the auditor doesn't acknowledge the span start; Caddy sends access logs to the auditor; the auditor alerts when a link has had no request for 5 minutes (`component_silent`) or when turns were recorded in the last 15 minutes and Caddy sent no access line (`caddy_silent`). Mismatch alerts stay deferred (phase 8) | A call is refused and recorded when the auditor doesn't acknowledge the span start; a silent component triggers an alert |
| Backup theft | Encrypted exports, a passphrase of at least 20 characters, and the auditor's alert on public key changes (`backup_key_changed`) | Restore without the passphrase fails; the admin can't read exports; a key change sends an alert |
| Vulnerable or changed dependency | Pinned images, scheduled CVE scans of dependencies (`cargo deny`) and images (Trivy, pinned by commit hash, with a dated ignore file), Dependabot alerts, and a release check; the CVE alert is GitHub's email for a failed scheduled run | A known vulnerable fixture fails `cargo deny`; the scheduled run's failure emails the owner |
| Malicious model output in a client or admin page | Block remote images and media; remote image syntax renders as a link; allow only http(s) links and show the full URL in a confirm bar before opening; `renderMarkdown` escapes all input first and adds only allowlisted markup; CSP with no inline scripts, plus Trusted Types required in the popup's and the admin's CSP | XSS and malicious Markdown payload library in CI, across client and admin views; a real-browser test confirms a malicious reply makes no network request and runs no script |
| Lost or stolen client | Token revocation and a client-held key | Revoked client is refused |
| Auditor down or disk full | Fail closed, reserved disk space, early alert (`audit_disk_low` below 1 GiB free, re-armed above 1.25 GiB; `audit_disk_critical` below 256 MiB) | Failure tests |
| Unwanted sealed content access | The rules in [0011](0011-sealed-content-access.md) | Each unseal writes an audit record and sends an alert; an unseal is refused if the record can't be written |

**Accepted for v1**

| Risk | Why it's accepted | Mitigation or later step |
| --- | --- | --- |
| Root access on the host | Host security is the admin's responsibility, and containers can't defend against root | Keep the host patched |
| Audit history rewritten by host root, or by a compromised admin and auditor together | With no off-host copy of the chain head, nothing outside the host can show it. The admin's chain head check catches an auditor-only change | Off-host audit hash after v1 |
| The whole system stops silently | With no dead-man monitor, nothing outside the host notices if the host or the `alerter` is down | The client shows "server offline"; check the stack by hand; dead-man monitor after v1 |
| Stolen host or disk | It depends on the hardware | Use disk encryption where practical |
| Container or kernel escape | Outside this project's control | Hardened containers and host patches |
| Compromised server container opens all space files | The server must read these files to answer users | Container hardening, no outbound route, and audit records |
| Compromised server misuses model access or sends data through model calls | The server must call `model-gateway`, and data can reach only configured providers | `model-gateway` budgets, model limits, kill switch, audit records, and budget alerts (the admin sends `budget_threshold` at 80% and 100% of the daily and monthly budget, once per period) |
| Compromised `model-gateway` or `cpa` sends data directly | The egress proxy is deferred | Egress proxy first after v1 |
| Compromised service uses its own token | Any service credential has this limit | One token per link, limited routes, and token rotation after a suspected compromise |
| Audit records older than 7 days age out if a compromised admin lowers retention | The admin controls retention above the 7-day floor | Retention changes are audited and send an alert; backups keep older copies until they expire |
| First-run takeover from a machine on a private network | First run follows the pfSense model | Private-address limit and forced password change |
| Admin can read family content | The owner's chosen design | Passphrase, records, alerts, notice |
| Providers see the prompts they receive | Required to use cloud models | Choose providers carefully, or self-host |
| Model text misleads a reader | AI replies can be wrong | Client warning |
| Lost passphrase | No recovery is the point | Password manager or printed copy |
| Hung service or failed admin | The controller is dropped | Docker restart policy, watchdog, host recovery |
| Provider, DNS, or home internet outage | No uptime promise | Provider list; manual recovery |
| `cpa` account or terms change | Third-party dependency | Keep `cpa` optional and keep direct providers working |

**Where each Defend row is tested.** `scripts/check-docs.sh` checks that every test named here exists (`git grep` for its name). `pending, PR <letter>` is allowed until the 1.0 release PR (PR AW); the letters are the PRs of the plan in [plan-v1.md](../plan-v1.md).

| Defend row | Tests |
| --- | --- |
| Admin or pairing brute force | `wrong_password_is_401_then_locks_out_even_the_right_one`, `wrong_keys_count_toward_the_same_lockout_as_wrong_passwords`, `a_wrong_username_and_a_wrong_password_get_the_same_answer_and_both_count_toward_the_lockout` |
| Compromised admin, which holds live secrets | `an_unseal_and_a_view_are_refused_when_their_record_cannot_be_written`, `a_key_cannot_open_a_session_change_the_password_or_stand_in_for_one`; pending, PR AB (fresh sign-in), PR V and PR W (alerts) |
| Unknown changes to the audit log | `a_shorter_chain_is_a_rollback_and_the_stored_head_stays_where_it_was`, `an_entry_with_another_hash_or_no_entry_at_all_is_a_rewrite`, `the_chain_head_moves_with_every_record_and_a_full_check_finds_an_edit` |
| False or missing records after a server compromise | `no_provider_is_called_when_the_auditor_refuses_and_the_refusal_is_recorded_later`, `an_acknowledgment_slower_than_a_second_is_a_refusal_and_no_provider_is_called`; pending, PR X (silence rules) |
| Backup theft | pending, PR AI, AJ, AL and AM |
| Vulnerable or changed dependency | pending, PR AR |
| Malicious model output in a client or admin page | pending, PR AE |
| Lost or stolen client | `revoked_token_is_401`, `a_client_the_snapshot_lists_as_revoked_is_refused` |
| Auditor down or disk full | `below_the_disk_floor_a_new_span_is_refused_but_the_records_of_open_spans_are_not`, `no_provider_is_called_when_the_auditor_cannot_be_reached`; pending, PR X (disk alerts), PR AT (drills) |
| Unwanted sealed content access | `an_unseal_and_a_view_are_refused_when_their_record_cannot_be_written`; pending, PR X (alerts) |

## Alternatives considered

Not recorded.

## Consequences

Each Defend control is a ship requirement. A new accepted risk needs the owner's approval and an update through the change rules in [0001](0001-decision-records-and-change-rules.md).

## Amendments

- 2026-11-08: marked as a register ADR, so it is amended in place ([0046](0046-component-and-register-adrs-and-partial-amendments.md)). Nothing else changed.
- 2026-11-11: design of "Caddy sends access logs to the auditor" (plan step 2.9, owner's choice of 2026-11-11; the built form is in [0051](0051-auditor.md)). Caddy's `net` log writer cannot carry a token, so the auditor takes the lines on an ingest listener of its own, on a new `internal` network `audit-caddy` shared by Caddy and the auditor alone, instead of a log-forwarding sidecar (which would be a new container and need its own ADR). The network is the only control on that listener: a compromised Caddy can append false lines but cannot read, change or delete anything. Access lines are best effort, not fail-closed like model calls: when the auditor is behind, lines are dropped and counted.
- 2026-11-11: controls decided for v1 (#70, owner's decisions; **decision changes** D17, D18, D20, D21 and D30): no passkeys (D20); the fresh sign-in list (D21); silence rules (D2, D3); Trusted Types required in both CSPs (D17); links and images (D18); CVE alerts by failed-run email and Trivy (D30); budget thresholds (D5); disk thresholds (D6); the sender of each alert (D1); and a table that maps each Defend row to its tests, checked by `check-docs.sh` (D36).
- 2026-11-11: the Defend test table names the existing tests for the rows that have them, and `scripts/check-docs.sh` checks that each named test exists and that every Defend row has a row (plan PR AU). No decision changed.
