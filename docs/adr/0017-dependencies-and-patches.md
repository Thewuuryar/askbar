# 0017. Dependencies and patches

Status: Accepted
Date: 2026-11-04
Plan decisions: 18

## Context

A deployer can't patch code they don't maintain, so maintainers and deployers need different duties. The admin also has no Docker access or outbound route. Askbar is licensed under `AGPL-3.0-or-later`, so code built into it must have a compatible license.

## Decision

| Duty | Project maintainer | Deployment admin |
| --- | --- | --- |
| First-party dependencies | Update, test, and release | Install the release |
| Third-party images such as `cpa` and Caddy | Review upstream changes, pin, test, and release | Install the release |
| Host and Docker | Publish minimum requirements | Patch them |
| Alerts | Provide rules and delivery options | Configure delivery and respond |
| Recovery | Document and test the steps | Keep backups and run recovery |

- **Pinning:** pin images and GitHub Actions to digests or commit hashes, the Caddy plugin to a version, and Rust and npm packages through committed lockfiles.
- **Updates:** Dependabot handles security alerts, and Renovate handles version update pull requests.
- **Checks:** require the `cargo deny` check and scheduled CVE scans.
- **Patch alerts:** a new release appears in the admin and sends an alert. `alerter` checks for new releases without sending user data, and the admin reads the result from it.
- **Installation:** the deployment admin pulls updated images from the host, or can automate this via a third party service. 
- **Migrations:** releases with a database migration need a backup first and are marked for manual installation.
- **Patch targets:** internet-facing critical issues within 7 days, and other security updates within 30 days.

**Licenses**
- CI checks Rust crates against a license allowlist with `cargo deny`, and npm packages with a similar check. `deny.toml` holds the one allowlist both use.
- A dependency outside the allowlist needs an ADR before it is added ([0001](0001-decision-records-and-change-rules.md) rule 3). A library inside it needs a reason in the pull request.
- The docs state that anyone who modifies Askbar and offers it over a network must follow the AGPL's source-offer requirement.

## Alternatives considered

- **Admin-managed updates through Docker access:** rejected, because it would bring back the risk removed in [0005](0005-no-controller-and-self-restart.md).
- **Reviewing licenses by hand:** rejected, because transitive dependencies are easy to miss.

## Consequences

Installing a patch takes a host command unless a deployer chooses automation. An image rollback doesn't undo a migration, so these releases need manual handling. An incompatible license fails CI before it is merged; the check doesn't replace legal advice, and third-party images still need their own license notices when published.

## Amendments

- 2026-11-10: merged in 0026 (dependency license check), as "Licenses", with its alternative and consequences.
