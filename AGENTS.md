# AGENTS.md

Rules for AI coding agents working on Askbar (Claude Code, Codex, Cursor, Copilot, others). Humans follow them too; the long form is [CONTRIBUTING.md](CONTRIBUTING.md).

"Agent" here means a coding agent working on this repo. Askbar itself has none: the model gets text in and returns text out (README), and stays that way through 1.0 ([ADR 0007](docs/adr/0007-conversation-only-and-tool-gating.md)). Do not add tools, a shell or file access for the model.

Items marked **(proposed)** await a decision; see [docs/plan-contributor-guidelines.md](docs/plan-contributor-guidelines.md).

## Precedence

When rules conflict, the higher one wins:

1. Security and audit invariants (below, [SECURITY.md](SECURITY.md)).
2. Architecture: [docs/architecture.md](docs/architecture.md) and accepted ADRs.
3. API contract: [docs/openapi/](docs/openapi/) and [docs/wire-contract.md](docs/wire-contract.md).
4. Style: [docs/style/](docs/style/).

If code and `docs/architecture.md` disagree, one is a bug. Fix it in the same commit; do not pick silently. If a rule is unclear or two rules clash, stop and ask.

## Commands

```sh
cargo build --workspace
cargo fmt --all --check                # (proposed) required once the rustfmt PR lands
cargo test --workspace                 # needs python3 (mock LLM)
npm install && npx playwright install chromium
npm test                               # names + docs checks, cargo tests, markdown, UI and admin browser tests
scripts/smoke.sh                       # end to end, no Docker
scripts/check-isolation.sh             # needs Docker; throwaway stack
```

Before you finish: `npm test` and `scripts/smoke.sh` pass. If you touched `compose.yaml`, a `Dockerfile`, Caddy, a network, a volume or anything about who can reach what, also run `scripts/check-isolation.sh`. If you cannot run something, say so in the PR; never imply it passed.

## Hard rules (MUST NOT)

- The server side never reads or writes the admin's data or settings, and never calls the admin. The admin pushes to the server over the server API only. A model, tool or reply must never be able to change settings.
- The admin stays in its own container and image. No admin code in the server image; the only things the two share are the `server-api` network and the `server-token` volume (a hash). The admin is served at `/admin` on the one host name; do not add a second domain or certificate.
- Never route `/api/server/*` or the controller publicly. Only Caddy publishes ports.
- The Docker socket stays in the controller, and the controller keeps its allow list of `server`, `caddy`, `cpa`. Do not widen it.
- Secrets never go in `.env`, logs, backups, error messages, test output or commits. Provider keys live encrypted in the server database and are never returned by any endpoint. An environment variable holds a path or an address, never a secret.
- Audit content stays sealed. The server never holds the audit passphrase or private key. Prompts, replies and provider errors appear nowhere except sealed audit payloads, not in logs or error text. Retention never goes below 7 days.
- Every path that calls a model or provider is recorded in the audit trace.
- Never rename a Docker volume that holds data. Never edit an earlier database migration; append.
- Within `v1`, never remove or rename a field, endpoint or error code. Add only.
- The admin site writes text with `textContent`, never `innerHTML`, and keeps its CSP. In the popup, only `renderMarkdown` output may reach `innerHTML`.
- Use the names in the [glossary](docs/architecture.md#glossary) (**client**, **server**, **admin**, **controller** and the rest). `scripts/check-names.sh` refuses the old words.
- Do not loosen or delete a test unless the contract changed, and say so in the commit message.
- Do not add a dependency without saying why in the PR. **(proposed)** `cargo deny check` passes.
- Do not change `.github/workflows/` or repository settings without asking.
- Do not push to `main`. Do not force-push a shared branch.

## How to work

- Read [docs/architecture.md](docs/architecture.md) before changing anything that crosses a crate, container, network or volume.
- Make the smallest change that does the job. One concern per commit. Tests and docs go in the same commit as the code.
- One place per fact: link to it, do not copy it.
- New endpoint: follow "Adding an endpoint, end to end" in CONTRIBUTING.md. The OpenAPI test fails if a route and its file disagree.
- Errors: return `ApiError` with an `ErrorCode`. Clients branch on `error`, never on prose.
- Logs and audit: follow [docs/style/logging-and-audit.md](docs/style/logging-and-audit.md).
- Names: follow [docs/style/naming.md](docs/style/naming.md). UI text and behaviour: [docs/style/ui-ux.md](docs/style/ui-ux.md).
- Report what you verified and what you did not, in the PR body.

## Stop and ask first

- A change that touches a hard rule above, a network, a volume, or who can reach what.
- A new dependency, a new container, a new published port, a new audience or API prefix.
- A decision that no doc or ADR records. Propose an ADR instead of deciding alone.
- A failing test you do not understand.

## Branches and commits **(proposed)**

- Branch: `<type>/<short-kebab-description>`, type one of `feat fix docs refactor test chore ci`. A harness-made `claude/...` branch is fine.
- Commit: [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): subject`. Imperative, lower case, no full stop, 72 characters or fewer. Scope is one of `proto server admin controller client ui compose docs ci deps`.
- Body: why, not what. List any deviation from the plan or any test you changed. Breaking change: `!` after the scope and a `BREAKING CHANGE:` footer.
- Pull request title is a commit subject. Fill in the PR template.
- Merging: squash by default. A stepwise series, where each commit stands alone, is rebase-merged; say so in the PR.
