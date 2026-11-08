# Architecture decision records

This set records the decisions from the original Askbar architecture plan (kept in git history as `docs/architecture-plan.md`, last at commit 1f47f86), plus the requirements and recommendations added afterward. The "Plan decisions" column gives the decision numbers from that plan. Every ADR starts as Proposed, because only the owner can move an ADR to Accepted ([0001](0001-decision-records-and-change-rules.md)).

| ADR | Title | Status | Plan decisions |
| --- | --- | --- | --- |
| [0001](0001-decision-records-and-change-rules.md) | Decision records and change rules | Accepted | 21 |
| [0002](0002-v1-scope-limits-and-ship-criteria.md) | V1 scope, limits, and ship criteria | Accepted | 1, 14, 15, 17, 23, 27 |
| [0003](0003-v1-service-layout.md) | V1 service layout | Accepted | 2, 5, 13 |
| [0004](0004-bootstrap-and-per-link-tokens.md) | Bootstrap and per-link tokens | Accepted | Bootstrap, optional CA |
| [0005](0005-no-controller-and-self-restart.md) | No controller, with self-restart | Accepted | 12, no controller, self-restart |
| [0006](0006-v1-outbound-access-and-alerts.md) | V1 outbound access and alerts | Accepted | 3, 33, no egress proxy in v1 |
| [0007](0007-first-run-and-admin-sign-in.md) | First run and admin sign-in | Accepted | 6, 7, 34, single admin |
| [0008](0008-users-clients-and-client-access.md) | Users, clients, and client access | Accepted | 28, 29, 31 |
| [0009](0009-spaces-and-storage.md) | Spaces and storage | Accepted | 8, 30 |
| [0010](0010-auditor.md) | Auditor | Accepted | 10, 11 |
| [0011](0011-sealed-content-access.md) | Sealed content access | Accepted | 11, 22 |
| [0012](0012-consent-notices-and-deletion.md) | Consent, notices, and deletion | Accepted | 22 |
| [0013](0013-backups.md) | Backups | Accepted | 9 |
| [0014](0014-model-gateway-and-providers.md) | `model-gateway` and providers | Accepted | 4, 20 |
| [0015](0015-certificates.md) | Certificates | Accepted | 20, 34 |
| [0016](0016-risk-register.md) | Risk register | Accepted | 16 |
| [0017](0017-dependencies-and-patches.md) | Dependencies and patches | Accepted | 18 |
| [0018](0018-releases-and-migrations.md) | Releases and migrations | Accepted | 19, 32 |
| [0019](0019-post-v1-order-and-tools.md) | Post-v1 order and tools | Accepted | 25, 26 |
| [0020](0020-remote-executor-and-ca-triggers.md) | Remote executor and CA triggers | Accepted | 24 |
| [0021](0021-names-and-new-components.md) | Names and new components | Superseded by 0038 | Added recommendation |
| [0022](0022-api-path-conventions.md) | API path conventions | Accepted | Added recommendation |
| [0023](0023-api-error-format.md) | API error format | Accepted | Added recommendation |
| [0024](0024-rejected-token-alert.md) | Rejected token alert | Accepted | Added recommendation |
| [0025](0025-audit-record-reliability.md) | Audit record reliability | Accepted | Added recommendation |
| [0026](0026-dependency-license-check.md) | Dependency license check | Accepted | Added recommendation |
| [0027](0027-adr-format-and-numbering.md) | ADR format and numbering | Accepted | Added recommendation |
| [0028](0028-service-boundaries-and-config-delivery.md) | Service boundaries and config delivery | Accepted | Required by v1 decisions |
| [0029](0029-shared-origin-protections.md) | Shared origin protections | Accepted | Required by v1 decisions |
| [0030](0030-sealed-audit-content.md) | Sealed audit content | Accepted | Required by v1 decisions |
| [0031](0031-audit-span-start-and-backlog.md) | Audit span start and backlog | Accepted | Required by v1 decisions |
| [0032](0032-container-hardening.md) | Container hardening | Accepted | Required by v1 decisions |
| [0033](0033-repository-layout.md) | Repository layout | Accepted | Owner request with the v1 plan |
| [0034](0034-self-signed-https-and-certificate-pinning.md) | Self-signed HTTPS and certificate pinning | Accepted | Required by 0007; owner choice in the v1 plan |
| [0035](0035-editorial-edits-to-accepted-adrs.md) | Editorial edits to accepted ADRs | Accepted | Owner review of the v1 layout change |
| [0036](0036-askbar-service-crate.md) | The `askbar-service` crate | Accepted | Plan step 1.3; owner rule for component ADRs |
| [0037](0037-askbar-cpa-image-and-supervisor.md) | The `askbar-cpa` image and its supervisor | Accepted | Plan step 1.4; owner choice of a supervisor for `cpa` |
| [0038](0038-names-and-new-components-without-a-name-check.md) | Names and new components, without a name check | Accepted | Owner decision; supersedes 0021 |
| [0039](0039-server.md) | The `server` component | Proposed | Plan step 1.14; owner rule for component ADRs |
| [0040](0040-admin.md) | The `admin` component | Proposed | Plan step 1.14; owner rule for component ADRs |
| [0041](0041-caddy.md) | The `caddy` component | Proposed | Plan step 1.14; owner rule for component ADRs |
| [0042](0042-cpa.md) | The `cpa` component | Proposed | Plan step 1.14; owner rule for component ADRs |
| [0043](0043-mock-llm.md) | The `mock-llm` component | Proposed | Plan step 1.14; owner rule for component ADRs |
| [0044](0044-desktop-client.md) | The desktop client | Proposed | Plan step 1.14; owner rule for component ADRs |
| [0045](0045-askbar-proto-crate.md) | The `askbar-proto` crate | Proposed | Plan step 1.14; owner rule for component ADRs |
