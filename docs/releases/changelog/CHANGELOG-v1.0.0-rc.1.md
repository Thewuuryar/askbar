# Changelog

All notable changes to Askbar are documented in this file. This one covers the changes since 0.6.0, the version of the crates before the release candidate. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Askbar adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0-rc.1] - 2026-11-13

### Added

- Scheduled encrypted backups go to a backup volume with retention, and a Backup page sets the destination, limits, reminders and a back-up-now button.
- Backups are encrypted bundles, made to a backup key that the admin creates, can rotate, and whose passphrase is set at setup; bundles can be downloaded from the admin site.
- A Restore page in the admin site restores an encrypted backup into an empty install, and is also offered on first run.
- Login files for the CPA helper and the alerter's state can be exported and imported by request file, and are included in the backup bundle.
- A new alerter service delivers alerts through ntfy and through email, with email settings encrypted beside the other alert settings.
- An Alerts panel in the admin site sets up alert delivery, sends a test alert, and shows the alerter on the Services card with a banner when delivery fails.
- The admin site raises alerts for rejected tokens, audit chain mismatches, restart loops, and budgets reaching 80 and 100 percent of a limit.
- The auditor raises alerts for an unseal, new unseal limits, a new backup key, a retention change, a low or critical audit disk, a silent link or Caddy, and an admin restart loop.
- The admin site counts repeated invalid tokens per link and flags a token that the server has stopped accepting.
- The alerter checks for new releases once a day.
- Every audit record is chained to the one before by hash, the auditor serves the chain head and a full chain check, and the admin site shows a banner when the chain does not match.
- The auditor runs as its own service, with acknowledged appends and group commit, and a provider call waits for its audit span to be acknowledged before it starts.
- The admin site records each change before making it and records each configuration push as an action, and services report the configuration version they applied.
- Caddy access logs are sent to the auditor and stored in the audit chain.
- The model gateway runs as its own service, makes provider calls with keys pushed from the admin site, and holds those keys in memory.
- Budgets, a kill switch and a per-user pause are held by the model gateway, which enforces them from token counts, and the admin site sets them.
- The admin site approves models and sets a default; the server offers only approved models, and the model gateway refuses any other.
- The admin site's provider test streams its reply and reports the time to first text.
- Users can pick a model for each chat in the desktop client.
- The admin site can delete a user, after the admin types the user's name to confirm.
- The admin site can pause and resume one user from the Users panel.
- Pasting a login address in the admin site matches it to the login's own redirect.
- The admin site sets a privacy notice and an AI warning, which the desktop client shows.
- The desktop client always shows its footer, which links to the source.
- Compose runs the auditor, the model gateway and the alerter as separate containers, each with its own networks and volumes.
- The Network page offers the HTTP-01 certificate challenge beside DNS, with a warning about what it needs.
- Release images are signed and published for main, release candidates and releases, and their digests are read back from the registry.
- CI runs a Trivy image scan and checks that cargo deny fails on a deliberately vulnerable fixture.

### Changed

- The admin site's audit tab reads the audit trail from the auditor instead of the server.
- The CPA login helper runs on the outbound network instead of the edge network.
- Rust CodeQL analysis runs on main and weekly rather than on every pull request, and pull request checks run faster.
- Passkey sign-in is not part of version 1.

### Removed

- The server's audit endpoints and its audit read link are removed; the auditor now holds the audit trail.
- The server no longer holds an in-process audit store.

### Fixed

- The desktop client builds on Windows and macOS again.
- The desktop client's lockfile includes its tracing dependency.
- The container image build includes the alerter's manifest.

### Security

- Changing the password, the API key, the retention, or deleting a user requires a fresh sign-in.
- The admin site opens sealed audit content only in a sealed-content session, which has its own panel, banner and timeouts.
- The popup and the admin site require Trusted Types, and the admin site's views are tested to stay inert.
- The popup renders replies safely, tested against 49 hostile Markdown payloads, and a hostile reply makes no request and runs no script.
- The popup shows the full address of a link before the link opens.
- The server has no outbound route, and provider keys are held only by the model gateway, pushed there from the admin site.
- Caddy's x/net dependency is raised to a fixed version.

[1.0.0-rc.1]: https://github.com/Thewuuryar/askbar/compare/904626f...v1.0.0-rc.1
