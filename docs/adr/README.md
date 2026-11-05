# Architecture decision records

This set records the decisions in the [Askbar Architecture Plan: Clean Copy](../architecture-plan.md), plus the requirements and recommendations added afterward. Every ADR is Proposed, because only the owner can move an ADR to Accepted ([0001](0001-decision-records-and-change-rules.md)).

| ADR | Title | Plan decisions |
| --- | --- | --- |
| [0001](0001-decision-records-and-change-rules.md) | Decision records and change rules | 21 |
| [0002](0002-v1-scope-limits-and-ship-criteria.md) | V1 scope, limits, and ship criteria | 1, 14, 15, 17, 23, 27 |
| [0003](0003-v1-service-layout.md) | V1 service layout | 2, 5, 13 |
| [0004](0004-bootstrap-and-per-link-tokens.md) | Bootstrap and per-link tokens | Bootstrap, optional CA |
| [0005](0005-no-controller-and-self-restart.md) | No controller, with self-restart | 12, no controller, self-restart |
| [0006](0006-v1-outbound-access-and-alerts.md) | V1 outbound access and alerts | 3, 33, no egress proxy in v1 |
| [0007](0007-first-run-and-admin-sign-in.md) | First run and admin sign-in | 6, 7, 34, single admin |
| [0008](0008-users-clients-and-client-access.md) | Users, clients, and client access | 28, 29, 31 |
| [0009](0009-spaces-and-storage.md) | Spaces and storage | 8, 30 |
| [0010](0010-auditor.md) | Auditor | 10, 11 |
| [0011](0011-sealed-content-access.md) | Sealed content access | 11, 22 |
| [0012](0012-consent-notices-and-deletion.md) | Consent, notices, and deletion | 22 |
| [0013](0013-backups.md) | Backups | 9 |
| [0014](0014-model-gateway-and-providers.md) | Model gateway and providers | 4, 20 |
| [0015](0015-certificates.md) | Certificates | 20, 34 |
| [0016](0016-risk-register.md) | Risk register | 16 |
| [0017](0017-dependencies-and-patches.md) | Dependencies and patches | 18 |
| [0018](0018-releases-and-migrations.md) | Releases and migrations | 19, 32 |
| [0019](0019-post-v1-order-and-tools.md) | Post-v1 order and tools | 25, 26 |
| [0020](0020-remote-executor-and-ca-triggers.md) | Remote executor and CA triggers | 24 |
| [0021](0021-names-and-new-components.md) | Names and new components | Added recommendation |
| [0022](0022-api-path-conventions.md) | API path conventions | Added recommendation |
| [0023](0023-api-error-format.md) | API error format | Added recommendation |
| [0024](0024-rejected-token-alert.md) | Rejected token alert | Added recommendation |
| [0025](0025-audit-record-reliability.md) | Audit record reliability | Added recommendation |
| [0026](0026-dependency-license-check.md) | Dependency license check | Added recommendation |
| [0027](0027-adr-format-and-numbering.md) | ADR format and numbering | Added recommendation |
| [0028](0028-service-boundaries-and-config-delivery.md) | Service boundaries and config delivery | Required by v1 decisions |
| [0029](0029-shared-origin-protections.md) | Shared origin protections | Required by v1 decisions |
| [0030](0030-sealed-audit-content.md) | Sealed audit content | Required by v1 decisions |
| [0031](0031-audit-span-start-and-backlog.md) | Audit span start and backlog | Required by v1 decisions |
| [0032](0032-container-hardening.md) | Container hardening | Required by v1 decisions |
