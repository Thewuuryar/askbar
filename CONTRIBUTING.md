# Contributing

How to build, run and change Askbar. How the parts fit and why is in [docs/architecture.md](docs/architecture.md); read that first. Decisions and their reasons are in [docs/adr/](docs/adr/). AI agents also follow [AGENTS.md](AGENTS.md); its rules apply to everyone.

## The names

The [glossary](docs/architecture.md#glossary) is the list. Everything, from paths to variable names to UI text, says **client** (a paired app and its token), **server** (the Askbar server) or **admin** (the admin site and its interface). Reviewers check for the retired names `mgmt`, `management` and `device` in Askbar's own roles; there is no script ([ADR 0038](docs/adr/0038-names-and-new-components-without-a-name-check.md)).

## Layout

| Path | What |
| --- | --- |
| `crates/askbar-proto/` | Types shared by everything: wire types, the error registry (`problem.rs`), URL prefixes (`paths.rs`). |
| `crates/askbar-service/` | What every service shares: the per-link bearer check, boot ID and `GET /status`, the `503 not_configured` gate. |
| `crates/askbar-audit/` | The audit store (`audit.db`): its migrations and the hash chain. |
| `services/server/` | The server: client API, server API, chat and generations, spaces. |
| `services/admin/` | The admin: its API, the site (`src/ui/`), Caddy configuration, API keys, backups. Unix only. |
| `services/auditor/` | The audit store's container (`/api/audit/v1`) and its alert rules. |
| `services/model-gateway/` | Provider keys (in memory) and every provider call; budgets, the kill switch, the model catalog. |
| `services/alerter/` | Alert delivery and the daily release check. |
| `services/bootstrap/` | Creates the per-link tokens on first start, then exits. |
| `services/cpa-supervisor/` | The supervisor in the `askbar-cpa` image: runs CLIProxyAPI, restarts it, and handles backup export and import ([ADR 0037](docs/adr/0037-askbar-cpa-image-and-supervisor.md)). |
| `clients/desktop/` | The desktop app, its own Cargo workspace: `src-tauri/` (package `askbar-client`, program `askbar`), the popup in `ui/` (no build step) and its `tests/`. |
| `compose.yaml`, `compose.staging.yaml`, `Dockerfile`, `deploy/` | The stack, the staging override, the images, and Caddy's bootstrap config. |
| `docs/` | `architecture.md`, `guide.md`, `adr/` (decisions), `style/`, `wire-contract.md`, `openapi/` (one file per API), `releases/`. |
| `scripts/`, `tests/` | Smoke, isolation and docs checks; `tests/e2e/` has the admin browser test and the mock model. |

The layout's rules are in [ADR 0033](docs/adr/0033-repository-layout.md). A new service needs a name and an ADR first ([ADR 0038](docs/adr/0038-names-and-new-components-without-a-name-check.md)).

## Build, run, test

```sh
cargo build --workspace            # server and admin (the app is built in clients/desktop)
cargo fmt --all --check            # CI runs it, here and in clients/desktop
cargo test --workspace             # needs python3 (a mock LLM)
npm install && npm --prefix clients/desktop install && npx playwright install chromium
npm test                           # names and link checks, cargo tests (both workspaces), markdown, UI and admin browser tests, mock LLM
scripts/smoke.sh                   # server + admin + mock LLM, end to end, no Docker
docker compose up -d --build && scripts/check-isolation.sh   # needs Docker; a throwaway stack. ASKBAR_SLOW_CHECKS=1 adds the five-minute alert check
python3 tests/e2e/mock_llm.py 8765     # a fake OpenAI-compatible server, to try things without a key
npm --prefix clients/desktop run dev   # the desktop app
```

The browser tests start the built `askbar-server` and `askbar-admin`; run `cargo build -p askbar-server -p askbar-admin` first after changing them (the admin site's files are compiled in).

**Third-party notices.** [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lists what ships and its licenses. After you change a dependency or an image's contents, run `scripts/third-party-notices.sh` and commit the result (`--check` says whether it is out of date). CI does not run it, because it needs the crates downloaded. The images section is written by hand in `scripts/third-party-notices.images.md`.

## Adding an endpoint, end to end

1. **Route.** Register it in the right table: `client_routes` in `services/server/src/lib.rs`, `routes()` in
   `services/server/src/server_api.rs`, `routes()` in `services/admin/src/lib.rs`, or `routes()` in the `src/lib.rs` of the auditor,
   `model-gateway` or alerter. Build the path from
   `askbar_proto::paths` (never a new literal); use plural nouns, and a sub-resource action
   (`POST .../cancel`) only for something that is not a plain create, update or delete. The rules are in
   [ADR 0022](docs/adr/0022-api-path-conventions.md).
2. **Who may call it.** Client API: behind the token and the version gate. Admin API: decide whether an API key may
   call it (the `signed_in_or_key` group) or only a person's session (`signed_in`: keys, the password, the session). An API key can never do the actions that need a fresh sign-in (set or rotate the backup key, restore, delete a user, lower retention, create a key, change the password).
3. **Errors.** Return `ApiError` (`askbar_proto::http`), never a bare `StatusCode`. Use an existing `ErrorCode` if one
   fits; if not, add it to `crates/askbar-proto/src/problem.rs` (codes are only ever added) with its status and title.
   Give a sentence for a person in `detail`.
4. **The OpenAPI entry.** Add the operation to the file for that API in `docs/openapi/`, with its responses. The test
   each crate's `tests/openapi.rs` fails if a route and the file disagree, in either direction, or if the `Problem` codes drift.
5. **Tests**, in the same commit as the code: the happy path, each refusal (check the `error` code, not the words),
   and the auth rule.
6. **Docs**, in the same commit: `docs/wire-contract.md` if the meaning needs words; `README.md` (the short
   version) or `docs/guide.md` (the details) if a person would notice. One place per fact: link, don't repeat.

From 1.0, within `/api/client/v1`, add only: never remove or rename a field, endpoint or error code (a hard rule in
[AGENTS.md](AGENTS.md)); anything else needs a `v2` beside it for a release. Paths follow [ADR 0022](docs/adr/0022-api-path-conventions.md).

## Database changes

Schema changes append to `MIGRATIONS` in `services/server/src/db.rs` or `services/admin/src/db.rs`; never edit an
earlier entry (its text is what made existing databases). Add a test that builds a database at the previous version,
puts rows in it and opens it with the new code; the existing ones show how. The audit store has its own
migrations in `crates/askbar-audit/src/lib.rs`.

Every space file runs the same migrations ([ADR 0009](docs/adr/0009-spaces-and-storage.md)). A release with a migration
needs a backup first and is installed by hand ([ADR 0017](docs/adr/0017-dependencies-and-patches.md),
[ADR 0018](docs/adr/0018-releases-and-migrations.md)).

Do not rename a Docker volume that holds data: Compose would create an empty one in its place. See the table in
`docs/architecture.md`.

## Writing your own admin UI, or scripting the admin

The admin API is public. Make a key in the admin site (the Security page; it is shown once), then:

```sh
curl -H "Authorization: Bearer $KEY" https://<host>/api/admin/v1/providers
```

A key can do everything the site can except manage keys, change the password, sign in or open sealed audit content. In a browser use the session:
`POST /api/admin/v1/session` with the password, then send the cookie (`__Secure-askbar_admin`, `Path=/api/admin`) and the header `X-Askbar-Admin: 1` on changes.
The whole API is [docs/openapi/admin.v1.yaml](docs/openapi/admin.v1.yaml).

## Conventions

- Don't loosen or delete a test unless the contract changed, and say so in the commit message.
- The admin site's text goes in through `textContent`, never `innerHTML` (its CSP forbids inline script and style).
- `askbar-server` and `askbar-admin` each have their own copy of the small route-table builder (`routes.rs`), so
  neither crate depends on the other. Keep them alike.
- A new library inside the license allowlist needs a reason in the PR. A new container, image or service, or a dependency outside the allowlist, also needs an ADR. The allowlist is
  the `allow` list in [deny.toml](deny.toml). `cargo deny check` (in the root and in `clients/desktop`) and the npm
  license check (`node scripts/check-npm-licenses.js package-lock.json clients/desktop/package-lock.json`) must
  pass ([ADR 0026](docs/adr/0026-dependency-license-check.md)). Commit both `package-lock.json` files with any
  npm change.
- Names, logs and audit, and UI text follow [docs/style/](docs/style/). The rules an agent must never break are the
  "Hard rules" in [AGENTS.md](AGENTS.md).

## Workflow

- **Branches.** `<type>/<short-kebab-description>`; type is one of `feat`, `fix`, `docs`, `refactor`, `test`, `chore`,
  `ci`. Branch from `main`; never push to it.
- **Commits.** [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): subject`, imperative, lower
  case, 72 characters or fewer. The scopes are listed in [AGENTS.md](AGENTS.md#branches-and-commits). The body says why, lists any deviation from a plan, and says when a test was changed. A breaking
  change has `!` after the scope and a `BREAKING CHANGE:` footer. One concern per commit; tests and docs ride with the code.
- **Pull requests.** The title is a commit subject. Fill in the template: what was verified, and what was not.
- **Merging.** Squash by default. A stepwise series where each commit stands alone is rebase-merged; say so in the PR.
- **A boundary change needs an ADR.** If a change moves a boundary (crate, container, network, volume, API audience),
  adds a container, image or service or a dependency outside the license allowlist, changes a hard rule in [AGENTS.md](AGENTS.md), or changes or reverses an ADR on `main`, write an
  ADR or amend one in place, with a line in its Amendments, in [docs/adr/](docs/adr/) in the same PR. Only the owner
  accepts an ADR or approves a change to a decision. A change to a security rule
  includes its docs and isolation tests in the same PR ([ADR 0001](docs/adr/0001-decision-records-and-change-rules.md)).
- **What CI runs for a pull request.** `lint` (which includes the docs and UI-token checks), `dependencies` and `secrets` always run. A pull
  request that changes only documentation skips `isolation`, `images`, `cpa-image` and `test-desktop`, and `test-services` too unless it
  touches `docs/openapi/`, `docs/wire-contract.md` or a file that is not Markdown. One that changes only `clients/desktop/` skips `isolation`,
  `images` and `cpa-image`. A skipped job counts as passing for the required checks. A push to `main`, a manual run and the weekly run run
  everything, including the five-minute alert check in `scripts/check-isolation.sh` (`ASKBAR_SLOW_CHECKS=1`). To change what is skipped, edit
  the `changes` job in `ci.yml`, never the required checks.
- **Review.** The owner reviews every pull request. Changes to the paths that decide who can reach what get extra care.

## Pre-built images

The images are published to the GitHub Container Registry. The packages are listed at <https://github.com/Thewuuryar?tab=packages>: `askbar-bootstrap`, `askbar-server`, `askbar-admin`, `askbar-auditor`, `askbar-model-gateway`, `askbar-alerter`, `askbar-caddy` and `askbar-cpa`.

They are private for now, so sign in first with a GitHub token that has `read:packages` (paste the token when asked):

```sh
docker login ghcr.io -u <your-github-user>
```

Pull one image, or set the registry and tag in `.env` and pull the whole stack:

```sh
docker pull ghcr.io/thewuuryar/askbar-server:main          # edge: the latest main
docker pull ghcr.io/thewuuryar/askbar-server:1.0.0-rc.1    # a release candidate

# in .env: ASKBAR_IMAGE_REGISTRY=ghcr.io/thewuuryar/  and  ASKBAR_IMAGE_TAG=1.0.0-rc.1
docker compose pull && docker compose up -d --no-build
```

Tags, release compose files and signature checks are in the guide ([Release channels](docs/guide.md#release-channels), [Using pre-built images](docs/guide.md#using-pre-built-images), [Verifying images and files](docs/guide.md#verifying-images-and-files)).

## Cutting a release candidate and a release

Only the owner pushes a release tag: the tag ruleset lets nobody else, and `images.yml` refuses a tag whose commit is not on `main`. The workflow never publishes a release; it drafts one.

You can also create the tag from the web page: **Releases > Draft a new release**, type the tag in *Choose a tag* (*Create new tag on publish*), tick **Set as a pre-release** for an rc, and publish. The tag run then replaces that release's text with `docs/releases/<tag>.md` and adds the files to it. The release is already public while the run works, so the files appear a few minutes later, and the repository's "immutable releases" setting must be off or the upload is refused.

1. **A release candidate.** On the `main` commit to release, with its `ci` run and its edge `images` run green: `git tag v1.0.0-rc.1 <commit> && git push origin v1.0.0-rc.1`. The `staging` environment has no reviewer. The run builds and signs the images, builds the installers, and drafts a **prerelease** holding `compose.release.yaml`, the installers, a tar of the SBOMs, and `SHA256SUMS` with its signature. Check the draft, and the signatures as the guide says ([Verifying images and files](docs/guide.md#verifying-images-and-files)).
2. **Staging.** Run the candidate as the guide says ([Staging a release](docs/guide.md#staging-a-release)) and complete the owner's checklist. A fix gets its own pull request, then a new candidate (`-rc.2`); never move a tag.
3. **The release.** Push `v1.0.0` on the commit that passed staging. The `production` environment waits: **Actions** → the run → *Review deployments* → tick `production` → *Approve and deploy*. The run drafts the release (with `docs/releases/v1.0.0.md` as its notes: write it, and its changelog in `docs/releases/changelog/`, before the tag; the run fails without it); read it, then publish the draft by hand.

An ignore for a vulnerable image package goes in `.trivyignore.yaml`, one with its ID, a statement and an expiry date
(`scripts/check-docs.sh` refuses one without them); an ignore for a vulnerable crate goes in `deny.toml`, with its ID, a reason and a
date to look again. `scripts/check-deny-fixture.sh` shows that `cargo deny` still fails on a crate pinned to a known-vulnerable release.

## Before you open a pull request

Run `npm test` and `scripts/smoke.sh`. If you touched `compose.yaml`, `Dockerfile`s, Caddy or anything about who can
reach what, run `scripts/check-isolation.sh` too (it needs Docker) and say that you did.
