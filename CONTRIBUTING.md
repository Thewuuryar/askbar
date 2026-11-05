# Contributing

Thanks for looking. This page is how to build, run and change Askbar. How the parts fit and why is in
[docs/architecture.md](docs/architecture.md); read that first. Decisions and their reasons are in
[docs/adr/](docs/adr/). If you are an AI agent, read [AGENTS.md](AGENTS.md) too; its rules apply to everyone.

## The names

The glossary in [docs/architecture.md](docs/architecture.md#glossary) is the list. Everything, from paths to variable names to UI text, says **client** (a paired app and its token), **server**
(the Askbar server, and its interface for the admin) or **admin** (the admin site and its interface). The old
words `mgmt`, `management` and `device` are checked for and refused by `scripts/check-names.sh`; a line that has <!-- names-ok: this sentence names them -->
to use one (a database migration, an old path in a test) carries `names-ok` and a reason.

## Layout

| Directory | What |
| --- | --- |
| `askbar-proto/` | Types shared by everything: wire types, the error registry (`problem.rs`), URL prefixes (`paths.rs`). |
| `askbar-server/` | The server: client API (`client_api.rs`), server API (`server_api.rs`), chat and generations, audit store. |
| `askbar-admin/` | The admin service: the admin API (`admin_api.rs`), the site (`src/ui/`), Caddy configuration, API keys. Unix only. |
| `askbar-controller/` | The controller: Docker-socket access narrowed to start, stop and restart three services. Its API is `docs/openapi/control.v1.yaml`. |
| `src-tauri/`, `ui/` | The desktop app (Cargo package `askbar-client`, program `askbar`) and its popup. No build step for `ui/`. |
| `docs/` | `architecture.md`, `adr/` (decisions), `style/`, `wire-contract.md` (prose), `openapi/` (one file per API), `refactor-log.md`. |
| `scripts/`, `tests/` | Smoke and isolation scripts, the checks, browser tests, a mock LLM. |

This table lists what is on `main`. The v1 target adds the `auditor`, `model-gateway`, `alerter` and `bootstrap` services and drops the controller ([ADR 0003](docs/adr/0003-v1-service-layout.md), [ADR 0005](docs/adr/0005-no-controller-and-self-restart.md)). A new service needs a name and an ADR first ([ADR 0021](docs/adr/0021-names-and-new-components.md)).

## Build, run, test

```sh
cargo build --workspace            # server, admin and the app's Rust side
cargo fmt --all --check            # (proposed) required once the rustfmt PR lands
cargo test --workspace             # needs python3 (a mock LLM)
npm install && npx playwright install chromium
npm test                           # names and link checks, cargo tests, markdown, UI and admin browser tests, mock LLM
scripts/smoke.sh                   # server + admin + mock LLM, end to end, no Docker
docker compose up -d --build && scripts/check-isolation.sh   # needs Docker; a throwaway stack
python3 tests/mock_llm.py 8765     # a fake OpenAI-compatible server to try things without a key
npm run dev                        # the desktop app
```

The browser tests start the built `askbar-server` and `askbar-admin`; run `cargo build -p askbar-server -p askbar-admin` first
when you have changed them (the admin site's files are compiled in).

## Adding an endpoint, end to end

1. **Route.** Register it in the right table: `client_routes` in `askbar-server/src/lib.rs`, `routes()` in
   `askbar-server/src/server_api.rs`, or `routes()` in `askbar-admin/src/lib.rs`. Build the path from
   `askbar_proto::paths` (never a new literal); use plural nouns, and a sub-resource action
   (`POST .../cancel`) only for something that is not a plain create, update or delete. The rules are in
   [ADR 0022](docs/adr/0022-api-path-conventions.md).
2. **Who may call it.** Client API: behind the token and the version gate. Admin API: decide whether an API key may
   call it (the `signed_in_or_key` group) or only a person's session (`signed_in`: keys, the password, the session).
3. **Errors.** Return `ApiError` (`askbar_proto::http`), never a bare `StatusCode`. Use an existing `ErrorCode` if one
   fits; if not, add it to `askbar-proto/src/problem.rs` (codes are only ever added) with its status and title.
   Give a sentence for a person in `detail`.
4. **The OpenAPI entry.** Add the operation to the file for that API in `docs/openapi/`, with its responses. The test
   `tests/openapi.rs` fails if a route and the file disagree, in either direction, or if the `Problem` codes drift.
5. **Tests**, in the same commit as the code: the happy path, each refusal (check the `error` code, not the words),
   and the auth rule.
6. **Docs**, in the same commit: `docs/wire-contract.md` if the meaning needs words; `README.md` if a person would
   notice. One place per fact: link, don't repeat.

After 1.0, changes within `/api/client/v1` are additive only (new fields, endpoints, codes); anything else needs a `v2`
beside it for a release. Before 1.0, paths can change to fit [ADR 0022](docs/adr/0022-api-path-conventions.md).

## Database changes

Schema changes append to `MIGRATIONS` in `askbar-server/src/db.rs` or `askbar-admin/src/db.rs`; never edit an
earlier entry (its text is what made existing databases). Add a test that builds a database at the previous version,
puts rows in it and opens it with the new code; the existing ones show how. The audit store has its own
migrations in `askbar-server/src/audit.rs`.

Every space file runs the same migrations ([ADR 0009](docs/adr/0009-spaces-and-storage.md)). A release with a migration
needs a backup first and is installed by hand ([ADR 0017](docs/adr/0017-dependencies-and-patches.md),
[ADR 0018](docs/adr/0018-releases-and-migrations.md)).

Do not rename a Docker volume that holds data: Compose would create an empty one in its place. See the table in
`docs/architecture.md`.

## Writing your own admin UI, or scripting the admin

The admin API is public. Make a key in the admin site (Security tab; it is shown once), then:

```sh
curl -H "Authorization: Bearer $KEY" https://<host>/admin/api/v1/providers
```

The examples use the path on `main`. Before 1.0 it moves to `/api/admin/v1` ([ADR 0022](docs/adr/0022-api-path-conventions.md)).
A key can do everything the site can except manage keys, change the password, sign in or open sealed audit content. In a browser use the session:
`POST /admin/api/v1/session` with the password, then send the cookie and the header `X-Askbar-Admin: 1` on changes.
The whole API is [docs/openapi/admin.v1.yaml](docs/openapi/admin.v1.yaml).

## Conventions

- Tests land in the same commit as the code they test. Don't loosen or delete a test unless the contract changed,
  and say so in the commit message.
- The admin site's text goes in through `textContent`, never `innerHTML` (its CSP forbids inline script and style).
- `askbar-server` and `askbar-admin` each have their own copy of the small route-table builder (`routes.rs`), so
  neither crate depends on the other. Keep them alike.
- Secrets never go in `.env`, logs or error messages, and go in backups only under the encryption in
  [ADR 0013](docs/adr/0013-backups.md).
- A new dependency needs a reason in the PR, and `cargo deny check` and the npm license check must pass
  ([ADR 0026](docs/adr/0026-dependency-license-check.md)).
- Names, logs and audit, and UI text follow [docs/style/](docs/style/). The rules an agent must never break are the
  "Hard rules" in [AGENTS.md](AGENTS.md).

## Workflow

- **Branches.** `<type>/<short-kebab-description>`; type is one of `feat`, `fix`, `docs`, `refactor`, `test`, `chore`,
  `ci`. Branch from `main`; never push to it.
- **Commits (proposed).** [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): subject`, imperative, lower
  case, 72 characters or fewer. The scopes are listed in [AGENTS.md](AGENTS.md#branches-and-commits). The body says why, lists any deviation from a plan, and says when a test was changed. A breaking
  change has `!` after the scope and a `BREAKING CHANGE:` footer. One concern per commit; tests and docs ride with the code.
- **Pull requests (proposed).** The title is a commit subject. Fill in the template: what was verified, and what was not.
- **Merging.** Squash by default. A stepwise series where each commit stands alone is rebase-merged; say so in the PR.
- **A boundary change needs an ADR.** If a change moves a boundary (crate, container, network, volume, API audience),
  adds a dependency or a container, changes a hard rule in [AGENTS.md](AGENTS.md), or reverses an accepted ADR, write or
  supersede one in [docs/adr/](docs/adr/) in the same PR. Only the owner accepts an ADR. A change to a security rule
  includes its docs and isolation tests in the same PR ([ADR 0001](docs/adr/0001-decision-records-and-change-rules.md)).
- **Review.** The owner reviews every pull request. Changes to the paths that decide who can reach what get extra care.

## Before you open a pull request

Run `npm test` and `scripts/smoke.sh`. If you touched `compose.yaml`, `Dockerfile`s, Caddy or anything about who can
reach what, run `scripts/check-isolation.sh` too (it needs Docker) and say that you did.
