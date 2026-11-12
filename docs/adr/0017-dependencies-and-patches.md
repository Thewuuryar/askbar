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
- **Checks:** require the `cargo deny` check and scheduled CVE scans: `cargo deny` for dependencies, and a Trivy scan of the images (pinned by commit hash, with an ignore file whose entries carry dates). A known-vulnerable fixture must fail `cargo deny`. The CVE alert is GitHub's email for a failed scheduled run. The owner turns on Dependabot alerts and branch protection in the repository settings.
- **Patch alerts:** a new release appears in the admin and sends an alert. `alerter` polls `https://api.github.com/repos/Thewuuryar/askbar/releases/latest` once a day with a fixed `User-Agent` and no install data (`release_available`), and the admin reads the result from it ([0055](0055-alerter.md)).
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
- 2026-11-11: CI and repository settings, and the release check (#70, D30 a decision change, D11): a Trivy image scan, a vulnerable fixture, the failed-run email as the CVE alert, Dependabot alerts turned on by the owner, and a daily poll of GitHub's latest-release API. Built in plan steps 4.3 and 7.1.
- 2026-11-12: built the release check (plan step 4.3, PR AA, #82): see [0055](0055-alerter.md). The request carries a fixed `User-Agent` and no install data; the answer is trusted only as a plain version; tested against a mock.
