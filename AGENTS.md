# AGENTS.md

Rules for AI coding agents working on Askbar (Claude Code, Codex, Cursor, Copilot, others). Humans follow them too; the long form is [CONTRIBUTING.md](CONTRIBUTING.md).

"Agent" here means a coding agent working on this repo. Askbar itself has none: the model gets text in and returns text out, and stays that way through 1.0 ([ADR 0002](docs/adr/0002-v1-scope-limits-and-ship-criteria.md)). Do not add tools, a shell or file access for the model.

This file, [SECURITY.md](SECURITY.md) and [docs/architecture.md](docs/architecture.md) describe the proposed v1 target. Not everything here is accepted or built yet, and the code on `main` still differs. Check each [ADR](docs/adr/README.md)'s status. Follow these rules for new work, and do not treat a difference from `main` as permission to change a boundary without the owner's approval.

Items marked **(proposed)** await a decision; see [docs/plan-contributor-guidelines.md](docs/plan-contributor-guidelines.md).

## Precedence

When rules conflict, the higher one wins:

1. Security and audit invariants (below, [SECURITY.md](SECURITY.md)).
2. Architecture: [docs/architecture.md](docs/architecture.md) and [ADRs](docs/adr/README.md).
3. API contract: [docs/openapi/](docs/openapi/) and [docs/wire-contract.md](docs/wire-contract.md).
4. Style: [docs/style/](docs/style/).

If code and `docs/architecture.md` disagree on behavior the target has already reached, one is a bug. Fix it in the same commit; do not pick silently. If the code hasn't reached the target yet, say so in the PR. If a rule is unclear or two rules clash, stop and ask.

## Commands

```sh
cargo build --workspace
cargo fmt --all --check                # (proposed) required once the rustfmt PR lands
cargo test --workspace                 # services and crates; needs python3 (mock LLM)
npm install && npm --prefix clients/desktop install && npx playwright install chromium
npm test                               # names + docs checks, cargo tests (root and clients/desktop), markdown, UI and admin browser tests
scripts/smoke.sh                       # end to end, no Docker
scripts/check-isolation.sh             # needs Docker; throwaway stack
```

Before you finish: `npm test` and `scripts/smoke.sh` pass. If you touched `compose.yaml`, a `Dockerfile`, Caddy, a network, a volume or anything about who can reach what, also run `scripts/check-isolation.sh`. If you cannot run something, say so in the PR; never imply it passed.

## Hard rules (MUST NOT)

- The server side never reads or writes the admin's data or settings, and never calls the admin. The admin pushes to the server over the server API only. A model, tool or reply must never be able to change settings.
- The admin stays in its own container and image. No admin code in the server image. The admin and server share only their authenticated API link and no data volumes. The admin site is at `/admin` and its API is at `/api/admin/v1` on the one host name; do not add a second domain or certificate.
- Never route `/api/server/*` or another internal API publicly. Only Caddy publishes ports.
- No service gets the Docker socket.
- Secrets never go in `.env`, logs, error messages, test output or commits. Backups contain secrets only under the encryption in [ADR 0013](docs/adr/0013-backups.md). The admin stores provider keys, `model-gateway` holds them in memory, and no endpoint returns them. An environment variable holds a path or an address, never a secret.
- Audit content stays sealed. The server and auditor never hold the audit passphrase or private key. Prompts, replies and provider errors appear in the audit store only as sealed payloads, and never in logs or error text. Retention never goes below 7 days.
- Every path that calls a model or provider is recorded in the audit trace. Only `model-gateway` opens model-call spans, and a provider call doesn't start until the auditor acknowledges its span.
- The admin records its own actions in the auditor, and refuses an action if its record can't be appended.
- Never rename a Docker volume that holds data. Never edit an earlier database migration; append.
- After 1.0, within `/api/client/v1`, never remove or rename a field, endpoint or error code. Add only.
- The admin site writes text with `textContent`, never `innerHTML`, and keeps its CSP. In the popup, only `renderMarkdown` output may reach `innerHTML`.
- Use the names in the [glossary](docs/architecture.md#glossary).
- Do not loosen or delete a test unless the contract changed, and say so in the commit message.
- Do not add a dependency without saying why in the PR. A new container, image or service, or a dependency outside the license allowlist, also needs an ADR ([ADR 0026](docs/adr/0026-dependency-license-check.md)). `cargo deny check` and the npm license check must pass.
- Do not change `.github/workflows/` or repository settings without asking.
- Do not push to `main`, merge a pull request, or accept an ADR without the owner's approval. Do not force-push a shared branch.

## How to work

- Read [docs/architecture.md](docs/architecture.md) before changing anything that crosses a crate, container, network or volume.
- Make the smallest change that does the job. One concern per commit. Tests and docs go in the same commit as the code.
- One place per fact: link to it, do not copy it.
- New endpoint: follow "Adding an endpoint, end to end" in CONTRIBUTING.md. The OpenAPI test fails if a route and its file disagree.
- Errors: return `ApiError` with an `ErrorCode`. Clients branch on `error`, never on prose ([ADR 0023](docs/adr/0023-api-error-format.md)).
- API paths: follow [ADR 0022](docs/adr/0022-api-path-conventions.md).
- Decisions and changes: follow [ADR 0001](docs/adr/0001-decision-records-and-change-rules.md).
- Logs and audit: follow [docs/style/logging-and-audit.md](docs/style/logging-and-audit.md).
- Names: follow [docs/style/naming.md](docs/style/naming.md). UI text and behaviour: [docs/style/ui-ux.md](docs/style/ui-ux.md).
- Report what you verified and what you did not, in the PR body.

## Stop and ask first

- A change that touches a hard rule above, a network, a volume, or who can reach what.
- A new dependency, a new container, a new published port, a new audience or API prefix.
- A decision that no doc or ADR records. Propose an ADR instead of deciding alone.
- A failing test you do not understand.

## Branches and commits

- Branch: `<type>/<short-kebab-description>`, type one of `feat fix docs refactor test chore ci`.
- Commit **(proposed)**: [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): subject`. Imperative, lower case, no full stop, 72 characters or fewer. Scope is one of `proto server admin auditor model-gateway alerter client ui compose docs ci deps`.
- Body **(proposed)**: why, not what. List any deviation from the plan or any test you changed. Breaking change: `!` after the scope and a `BREAKING CHANGE:` footer.
- Pull request title **(proposed)** is a commit subject. Fill in the PR template.
- Merging: squash by default. A stepwise series, where each commit stands alone, is rebase-merged; say so in the PR.
