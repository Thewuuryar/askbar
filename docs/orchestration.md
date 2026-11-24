# How plans run

Every plan with numbered steps (the files `docs/plan-*.md` and `docs/plan-tiered-routing/*.md`) runs this way. A plan may add its own rules in a "How to run" section; those add to this file and never loosen it. The work order is in [plan-priorities.md](plan-priorities.md).

## Starting a session (owner)

In the repository root, start Claude Code on Sonnet (`claude --model sonnet`, or `/model sonnet` in a running session). Accept the workspace trust prompt: the repository's skills in `.claude/skills/` (including `ponytail`) load only after it. Connect the Graphify MCP server for `Thewuuryar/askbar` if you have it. Then send:

> You are the orchestrator for `docs/<plan file>`. Read `docs/orchestration.md` and the plan file, then follow the plan from the first unchecked step.

Start a new session for each stage or phase, or when the context is long (`/context` shows it). The ticked boxes carry the state.

## Roles

- **Orchestrator.** It always runs on Sonnet, in the owner's session. It reads the plan, briefs subagents, checks, reviews, commits and ticks. It never writes step code itself.
- **Subagents.** One fresh subagent per step, on the model the step names. Use Haiku unless the step needs Sonnet (see [Haiku or Sonnet](#haiku-or-sonnet)). Never fork the orchestrator's context into a subagent.
- **Owner.** Owns gates, ADR acceptance, merges and `.github/`.

## Loop

1. Read this file and the plan file once. Pick the first unchecked step whose `after` steps are ticked and whose gate is cleared. If the gate is open, stop and ask the owner. Never skip ahead past a gate.
   - To check an `after` step in another file, grep for it: `grep -rn '\*\*R1.3\*\*' docs/` (a ticked step starts `- [x]`).
   - Do not read other plan files, ADRs or design files unless the step cites them, and then only the cited part.
2. Make one branch per stage or phase, named `<type>/<kebab>` as the plan says. Create it from `main`, or from the previous stage's branch while that PR is open, and say so in the PR.
3. Spawn the step's subagent in the foreground with the [brief](#subagent-brief). One subagent per step. A run of consecutive docs-only Haiku steps in one stage may go to one Haiku agent, which still makes one commit per step.
4. When the subagent hands back, check and review:
   - Run the [checks](#checks) for the step type.
   - Read `git show --stat HEAD`, then the diff file by file (`git show HEAD -- <path>`), against the step's Accept line and the [review list](#review-list). Skip lockfiles and generated fixtures.
   - Send fixes back to the same agent (`SendMessage`), which amends its commit.
5. The agent commits with the step's exact subject. Check the body: why, deviations, any test changed and why, and the attribution lines your session was given. Tick the step's box in the same commit.
   - Tick the step's line in [task-tracker.md](task-tracker.md) in the same commit.
6. Push the branch after each step with `git push -u origin <branch>`. Never force-push, never push to `main`, never merge.
7. At stage end:
   - Run the full checks.
   - Open one PR titled with the stage's subject and fill the template.
   - Say what ran and what did not, and say "rebase-merge, each commit stands alone".
   - Stop for the owner.

## Keep context small

The orchestrator pays for everything it reads and everything it sends.

**Do:**
- Brief with the step block verbatim, the file list and the rules below.
- Name a file range (`path:120-180`) or a grep command, not the file's text.
- Ask for a short final report (at most 15 lines).
- Read check output through `tail`, as shown under [Checks](#checks).
- Review diffs one file at a time.

**Don't:**
- Paste plan files, ADRs, earlier diffs, logs or earlier reports into a brief.
- Re-read a file after an edit you made.
- Re-run a slow suite the agent already reported passing, unless the report is incomplete.

## Subagent brief

Keep it this small:

1. The step block, verbatim.
2. "Read `AGENTS.md`. Load the `ponytail` skill (`/ponytail`; it is in `.claude/skills/ponytail/`) and follow it for all code you write."
3. "Read only the files in the step's Files line, and only the parts you need. To understand code first, follow [Reading the code](#reading-the-code)."
4. "Make one commit with this exact subject. Tests and docs go in the same commit. Do not push. Do not add dependencies. Do not touch `.github/`. Stop and report if the step needs a decision the plan does not make."
5. The check commands for the step type, run as shown under [Checks](#checks).
6. "Final report, at most 15 lines: commit hash, files changed, check results, deviations, what you did not verify."

## Reading the code

Agents read the code they change; they don't read the codebase.

1. **Graphify first**, when its tools are present. Load them with `ToolSearch` (`+Graphify`) and pass `repository_id: "Thewuuryar/askbar"`.
   - A file's outline: `graphify_file_neighbors` with `symbols_only: true`.
   - Who calls something: `graphify_callers`.
   - What a change reaches: `graphify_impact`.
   - Where a behaviour lives: `query_graph` with `skeleton: true`.
   - A symbol's body: `graphify_node`.
   - If the result's `commitSha` differs from `git rev-parse HEAD`, read the files changed since that commit directly.
2. **Then grep and read ranges.** Use `grep -n` to find lines and `Read` with an offset and limit. Do not read a file over 300 lines whole.
3. **Without Graphify**, use step 2 only. Never sweep the tree file by file.

## Checks

Run each command as `<command> > /tmp/check.log 2>&1; echo "exit $?"; tail -n 30 /tmp/check.log`. Read more of the log only when it fails.

| Touches | Run |
| --- | --- |
| Rust crate or service | `cargo fmt --all --check`, `cargo clippy -p <crate> --all-targets --locked -- -D warnings`, `cargo test -p <crate>` (OpenAPI tests run with it) |
| `clients/desktop` | The same three inside `clients/desktop`, then `npm test` |
| Admin UI (`services/admin/src/ui`) | `cargo test -p askbar-admin`, then `npm test` |
| Docs or ADR only | `scripts/check-docs.sh` |
| Dependency or crate feature added | `cargo deny check`; `node scripts/check-npm-licenses.js package-lock.json clients/desktop/package-lock.json` |
| `compose.yaml`, Dockerfile, network, volume, `scripts/check-isolation.sh` | `scripts/check-isolation.sh` (needs Docker) |
| Stage end | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace`, the same three in `clients/desktop` if touched, `npm test`, `scripts/smoke.sh`, plus the rows above |

## Review list

- **Secrets and text:** no prompt text, reply text, provider key or signing key in logs, errors, `/status` or audit attributes. Only sealed payloads carry text.
- **Audit:** every model or tool call has its audit span acknowledged before the call.
- **Compatibility:** new fields are additive with `#[serde(default)]`, and old payloads still parse.
- **Constants:** no literal behaviour values (limits, intervals, thresholds, defaults) in service code. They live in `services/admin/src/constants.rs`, with bounds in `askbar-proto` when a service also checks them.
- **Migrations and tests:** no earlier migration edited, and no test loosened without a stated contract change.
- **Ponytail:**
  - no wrapper, option or abstraction the step didn't need;
  - deletion preferred over addition;
  - one concern per commit;
  - new non-trivial logic has a test.
- **Docs:** OpenAPI and docs are updated with the code.

## Haiku or Sonnet

Default to Haiku. A step names its model, and the orchestrator may move a Haiku step to Sonnet, never the reverse.

| Model | Use for |
| --- | --- |
| Haiku | Docs, ADR amendment text, OpenAPI and proto boilerplate, test fixtures and helpers, mock extensions, grep or explore tasks, moving code without changing it, mechanical renames and dedupes, deleting dead code, pure functions with a given formula, markup and CSS |
| Sonnet | Gateway, server and admin logic; anything concurrent; security, crypto or audit-path code; anything with a migration; isolation checks; any step where Haiku already failed |
| Owner | Gates, ADR acceptance, merges, `.github/` |

If a Haiku step turns out to need judgment, move it to Sonnet. Do not ask Haiku twice.

## Retries

- When a check fails, the same agent gets one fix attempt.
- If it still fails:
  - a Haiku step goes to a fresh Sonnet agent, with only the diff and the failing output;
  - a Sonnet step stops, and the orchestrator asks the owner.
- An unexplained failing test stops the step at once.

## Stop and ask

Stop and ask the owner before any of these:
- a new dependency, or one outside the licence allowlist;
- a new container, port, network or volume;
- any change to a hard rule in `AGENTS.md`;
- a missing or unaccepted ADR;
- a decision the plan marks "assumed" that the work contradicts;
- an unexplained failing test.

Never accept an ADR, merge, or batch ADR amendments. One amendment is one commit, `docs(adr): amend NNNN: <what>`.

## Compatibility rules

- Proto and API changes are additive: `#[serde(default)]`, optional fields, no renames.
- An old server with a new gateway keeps working, and the reverse. A test parses the old payload shape.
- Never rename a volume. Append migrations; never edit an earlier one.
- Pushed constants are additive and defaulted. A service uses the `askbar-proto` defaults until its first push.
