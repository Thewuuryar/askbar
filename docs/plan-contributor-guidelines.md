# Plan: contributor and agent guidelines

Status: draft for review. Nothing here is enforced until the matching step in "Rollout" lands.

## What this adds

| File | For | Says |
| --- | --- | --- |
| [AGENTS.md](../AGENTS.md), `CLAUDE.md` | agents | short imperative rules, commands, hard invariants, when to stop and ask |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | everyone | existing build and change guide, plus a new "Workflow" section |
| [docs/style/](style/) | everyone | naming, logging and audit, UI and UX |
| [docs/adr/](adr/) | everyone | why the boundaries are where they are; two proposed decisions |
| `.github/pull_request_template.md`, `.github/CODEOWNERS` | PRs | what a PR must state; who reviews sensitive paths |

## Already decided (and where it is recorded now)

| Decision | Source on `main` | Recorded in |
| --- | --- | --- |
| Thin clients; one server holds history and settings | README, architecture.md | architecture.md |
| Three words plus controller, enforced by script | architecture.md, `check-names.sh` | [naming.md](style/naming.md), [ADR 0002](adr/0002-three-audiences-one-host.md) |
| One host; admin at `/admin`; audiences by path | architecture.md | ADR 0002 |
| Admin and server isolated; one-way config push | architecture.md, SECURITY.md | [ADR 0003](adr/0003-admin-and-server-isolation.md), AGENTS.md |
| Audit content sealed; keep 30 days, floor 7 | README | [ADR 0004](adr/0004-sealed-audit-content.md), [logging-and-audit.md](style/logging-and-audit.md) |
| Docker socket only in the controller | SECURITY.md, plan-admin-managed-setup.md | [ADR 0005](adr/0005-docker-socket-only-in-controller.md) |
| Problem-details errors, stable codes, add-only | architecture.md, `problem.rs` | [ADR 0006](adr/0006-problem-details-errors.md) |
| ULIDs, UTC unix seconds, `/api/client/v1` paths | wire-contract.md | naming.md |
| Migrations append-only; volumes never renamed | CONTRIBUTING.md | AGENTS.md |
| Tests and docs in the same commit; one place per fact | CONTRIBUTING.md | AGENTS.md |
| Secrets never in `.env`, logs, backups, errors | CONTRIBUTING.md, README | AGENTS.md, logging-and-audit.md |
| Admin text via `textContent`, strict CSP; popup has no build step | CONTRIBUTING.md | [ui-ux.md](style/ui-ux.md) |
| Per-user token limits and kill switch; CLIProxyAPI as a managed provider | README | not a guideline; shipped |
| AGPL-3.0-or-later | LICENSE | not changed |

## Where the goals document and `main` differ

| Goals document | `main` | Handled by |
| --- | --- | --- |
| Admin on `admin.askbar...` subdomain, desktop only | `/admin` on one host, optional allowlist | [ADR 0008](adr/0008-admin-sign-in-and-exposure.md) (proposed) |
| Passkey login, password and TOTP as backup | password and an admin API key | ADR 0008 |
| Native Swift iPhone app | not started; API already allows it | ui-ux.md "Other clients" (proposed) |
| "Log everything agents do"; web research and code execution later | README: conversation only, no tools | [ADR 0007](adr/0007-conversation-only-and-tool-gating.md) (proposed) |
| Audit kept at least 7 days, only admin reads | sealed content, 30 days default, floor 7 | `main` is stricter; adopted |
| Self-improving memory, projects, NAS, Tailscale | listed under "Not built yet" | no guideline needed yet |

## Open decisions

Recommendation first. "Tool" lists the usual choices; check each one's current state before adopting.

| # | Question | Recommendation | Tool |
| --- | --- | --- | --- |
| 1 | Commit format | Conventional Commits, scopes = crate or area (see AGENTS.md). Keeps a changelog and version bump automatable later | `commitlint`, `cocogitto` |
| 2 | Branch names | `<type>/<kebab>`; accept `claude/*` from the harness | a CI check on the branch name |
| 3 | Merge strategy | Squash for small PRs, rebase-merge for a stepwise series (as the 0.5.0 refactor was). PR title is a commit subject | GitHub repository setting |
| 4 | rustfmt | Adopt a `rustfmt.toml` and reformat once, as its own commit listed in `.git-blame-ignore-revs`. See the data below | `rustfmt` |
| 5 | clippy | Put lints in `[workspace.lints]`; fix the baseline; then `-D warnings` in CI | `clippy` |
| 6 | Where the gate lives | CI is the gate. Local hooks are optional speed-ups; agents can skip them | `lefthook`, `pre-commit` |
| 7 | CI triggers | Add a workflow on `pull_request`: fmt, clippy, `npm test`, smoke. Run `check-isolation.sh` there too (GitHub's Ubuntu runners have Docker). Keep the macOS and Windows matrix manual or on release tags | GitHub Actions |
| 8 | Dependencies | `cargo deny` for advisories, licences (this is AGPL) and sources; Dependabot for cargo, npm, Docker and Actions | `cargo-deny`, Dependabot |
| 9 | Secret scanning | gitleaks in CI, plus GitHub push protection if the repo is public | `gitleaks` |
| 10 | Branch protection | A ruleset on `main`: PR required, status checks required, no force push, no deletion. Zero required approvals while you are the only owner; raise it when others join | GitHub rulesets |
| 11 | AI attribution | Keep a `Co-authored-by:` trailer on agent commits so history shows it | none |
| 12 | Docs checks | Add `AGENTS.md` to `scripts/check-docs.sh` (done in this branch); optionally `lychee` for external links | `lychee`, `markdownlint-cli2` |
| 13 | `unsafe` | No `unsafe` exists in the workspace today. Set `unsafe_code = "forbid"` for the four server-side crates; try the client first, since macros can expand to it | workspace lints |
| 14 | Swift app conventions | Decide when the app starts: `swift-format` or `SwiftLint`, same error-code mapping as the desktop app | |
| 15 | Accessibility baseline for the popup | Not decided anywhere. Suggest: every action reachable by keyboard (already true), visible focus, contrast checked in both themes | |

### Data for #4 and #5 (measured on `main` at `7b76842`)

- `cargo fmt --check` with no config reports **1,593** diffs. 899 lines are over 120 columns and 159 are over 160, so the code was never run through rustfmt at the default width.

| rustfmt settings | diffs |
| --- | --- |
| default | 1,593 |
| `max_width = 140` | 1,082 |
| `max_width = 160` | 842 |
| `max_width = 140`, `use_small_heuristics = "Max"` | 464 |
| `max_width = 160`, `use_small_heuristics = "Max"` | 332 |

  No setting reaches zero, so any adoption is one large reformat commit. Do it when no other branch is open.
- `cargo clippy --workspace --all-targets` on the four server-side crates (`askbar-proto`, `-server`, `-admin`, `-controller`) gives about **44 warnings**: 20 "`Err` variant is very large", 16 "`ok().expect()` on a `Result`", and 8 one-offs. `src-tauri` was not checked here (it needs GTK libraries).

## Rollout

Each step is its own PR. Stop after any step; the earlier ones stand alone.

1. **This branch**: the documents, PR template, CODEOWNERS, `check-docs.sh` change. Verify: `npm test` names and docs checks pass.
2. **rustfmt** (#4): `rustfmt.toml`, one `style: apply rustfmt` commit, `.git-blame-ignore-revs`. Verify: `cargo fmt --check` clean; `cargo test --workspace` unchanged.
3. **clippy** (#5, #13): `[workspace.lints]`, fix or explicitly allow each baseline warning with a reason. Verify: `cargo clippy --workspace --all-targets -- -D warnings` clean.
4. **CI on pull requests** (#7, #8, #9): `ci.yml`, `deny.toml`, Dependabot, gitleaks. Verify: open a draft PR and watch it run; the 0.5.0 hand-off notes that `build.yml` has never run, so expect first-run failures.
5. **Branch protection** (#10) last, once the checks are known to pass; otherwise it blocks every merge.
6. **Update AGENTS.md** to drop the `(proposed)` tags and add the new commands.

## Keeping it alive

Add a rule only after something went wrong or was debated, and add the check that catches it in the same change. A rule with no command that verifies it belongs in a review checklist, not in AGENTS.md.
