# 0017. Dependencies and patches

Status: Accepted 
Date: 2026-11-04
Plan decisions: 18

## Context

A deployer can't patch code they don't maintain, so maintainers and deployers need different duties. The admin also has no Docker access or outbound route.

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

## Alternatives considered

- **Admin-managed updates through Docker access:** rejected, because it would bring back the risk removed in [0005](0005-no-controller-and-self-restart.md).

## Consequences

Installing a patch takes a host command unless a deployer chooses automation. An image rollback doesn't undo a migration, so these releases need manual handling.

## Amendments
