# Plan: contributor and agent guidelines

Status: tracker for decisions not yet made and work not yet done. Decisions that are made live in the [ADRs](adr/README.md), [AGENTS.md](../AGENTS.md) and [CONTRIBUTING.md](../CONTRIBUTING.md). Nothing here is enforced until the matching step in "Rollout" lands.

## Decided in review

- Merging: squash small PRs; rebase-merge a stepwise series and say so in the PR (2026-11-03).
- rustfmt now: `max_width = 140`, `use_small_heuristics = "Max"`, one reformat commit (branch `style/rustfmt`) (2026-11-03).
- Branch names: `<type>/<short-kebab-description>` for everyone, including agent harnesses (2026-11-05).

## Open decisions

Recommendation first. "Tool" lists the usual choices; check each one's current state before adopting. #2, #3 and #4 are decided (above).

| # | Question | Recommendation | Tool |
| --- | --- | --- | --- |
| 1 | Commit format | Conventional Commits, scopes = crate or area (see AGENTS.md). Keeps a changelog and version bump automatable later | `commitlint`, `cocogitto` |
| 2 | Branch names | `<type>/<kebab>` for everyone, with no exception for harnesses | a CI check on the branch name |
| 3 | Merge strategy | Squash for small PRs, rebase-merge for a stepwise series (as the 0.5.0 refactor was). PR title is a commit subject | GitHub repository setting |
| 4 | rustfmt | Adopt a `rustfmt.toml` and reformat once, as its own commit listed in `.git-blame-ignore-revs`. See the data below | `rustfmt` |
| 5 | clippy | Put lints in `[workspace.lints]`; fix the baseline; then `-D warnings` in CI | `clippy` |
| 6 | Where the gate lives | CI is the gate. Local hooks are optional speed-ups; agents can skip them | `lefthook`, `pre-commit` |
| 7 | CI triggers | Add a workflow on `pull_request`: fmt, clippy, `npm test`, smoke. Run `check-isolation.sh` there too (GitHub's Ubuntu runners have Docker). Keep the macOS and Windows matrix manual or on release tags | GitHub Actions |
| 8 | Dependencies | Decided in [ADR 0017](adr/0017-dependencies-and-patches.md) and [ADR 0026](adr/0026-dependency-license-check.md): `cargo deny` and an npm license check, Dependabot for security alerts, Renovate for version updates. Only the rollout is open | `cargo-deny`, Dependabot, Renovate |
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
- `cargo clippy --workspace --all-targets` on the four server-side crates as of 2026-11 (`askbar-proto`, `-server`, `-admin`, `-controller`; the controller has since been removed) gives about **44 warnings**: 20 "`Err` variant is very large", 16 "`ok().expect()` on a `Result`", and 8 one-offs. `src-tauri` was not checked here (it needs GTK libraries).

## Rollout

Each step is its own PR. Stop after any step; the earlier ones stand alone.

1. **This branch**: the documents, PR template, `check-docs.sh` change. There is no CODEOWNERS file while the owner is the only reviewer; add one with a second reviewer. Verify: `npm test` names and docs checks pass.
2. **rustfmt** (#4): branch `style/rustfmt` off `main`: `rustfmt.toml` and one `style: apply rustfmt` commit. Verify: `cargo fmt --all --check` clean; tests unchanged. After it merges, add the SHA that landed on `main` (a squash or rebase changes the SHA) to `.git-blame-ignore-revs` in a small follow-up.
3. **clippy** (#5, #13): `[workspace.lints]`, fix or explicitly allow each baseline warning with a reason. Verify: `cargo clippy --workspace --all-targets -- -D warnings` clean.
4. **CI on pull requests** (#7, #8, #9): `ci.yml`, `deny.toml`, Dependabot, Renovate, gitleaks. Verify: open a draft PR and watch it run; the 0.5.0 hand-off notes that `build.yml` has never run, so expect first-run failures.
5. **Branch protection** (#10) last, once the checks are known to pass; otherwise it blocks every merge.
6. **Update AGENTS.md** to drop the `(proposed)` tags and add the new commands.
7. **Delete this file** once steps 2 to 6 are done and the open decisions are settled or moved to issues.

## Keeping it alive

Add a rule only after something went wrong or was debated, and add the check that catches it in the same change. A rule with no command that verifies it belongs in a review checklist, not in AGENTS.md.
