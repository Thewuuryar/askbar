# 0016. Risk register

Status: Accepted
Date: 2026-11-04
Plan decisions: 16

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
| Admin or pairing brute force | Recommended passkeys, allowlist, rate limits, attempt cap, identical errors | Limit and lockout tests |
| Compromised admin, which holds live secrets | Fresh sign-in for sensitive changes, independent audit, alerts | Admin can't alter records; alerts fire |
| Unknown changes to the audit log | Hash chain, with the admin checking the latest chain head against the auditor ([0025](0025-audit-record-reliability.md)) | Tamper test |
| False or missing records after a server compromise | `model-gateway` records each call and refuses the call when the auditor doesn't acknowledge the span start; Caddy sends access logs to the auditor; alerts when a component that appends goes silent. Add mismatch alerts if time allows | A call is refused and recorded when the auditor doesn't acknowledge the span start; a silent component triggers an alert |
| Backup theft | Encrypted exports, a passphrase of at least 20 characters, and alerts on public key changes | Restore without the passphrase fails; the admin can't read exports; a key change sends an alert |
| Vulnerable or changed dependency | Pinned images, scheduled CVE scans of dependencies and images, and alerts on new CVEs and dependency updates | A known vulnerable test dependency fails CI and sends an alert |
| Malicious model output in a client or admin page | Block remote images and media; allow only safe links and show the destination before opening; `renderMarkdown` escapes all input first and adds only allowlisted markup; CSP with no inline scripts, plus Trusted Types where supported | XSS and malicious Markdown payload library in CI, across client and admin views; a real-browser test confirms a malicious reply makes no network request and runs no script |
| Lost or stolen client | Token revocation and a client-held key | Revoked client is refused |
| Auditor down or disk full | Fail closed, reserved disk space, early alert | Failure tests |
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
| Compromised server misuses model access or sends data through model calls | The server must call `model-gateway`, and data can reach only configured providers | `model-gateway` budgets, model limits, kill switch, audit records, and budget alerts |
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

## Alternatives considered

Not recorded.

## Consequences

Each Defend control is a ship requirement. A new accepted risk needs the owner's approval and an update through the change rules in [0001](0001-decision-records-and-change-rules.md).
