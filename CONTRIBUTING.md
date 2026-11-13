# Contributing

Thanks for looking. This page is how to build, run and change Askbar. How the parts fit and why is in
[docs/architecture.md](docs/architecture.md); read that first. Decisions and their reasons are in
[docs/adr/](docs/adr/). If you are an AI agent, read [AGENTS.md](AGENTS.md) too; its rules apply to everyone.

## The names

The glossary in [docs/architecture.md](docs/architecture.md#glossary) is the list. Everything, from paths to variable names to UI text, says **client** (a paired app and its token), **server**
(the Askbar server, and its interface for the admin) or **admin** (the admin site and its interface). Reviewers
check for the retired names `mgmt`, `management` and `device` in Askbar's own roles; there is no script
([ADR 0038](docs/adr/0038-names-and-new-components-without-a-name-check.md)).

## Layout

| Directory | What |
| --- | --- |
| `crates/askbar-proto/` | Types shared by everything: wire types, the error registry (`problem.rs`), URL prefixes (`paths.rs`). |
| `crates/askbar-service/` | What every service shares: the per-link bearer check (`links.rs`), boot ID and `GET /status` body (`status.rs`), the `503 not_configured` gate (`configured.rs`). |
| `services/server/` | The server: client API (`client_api.rs`), server API (`server_api.rs`), chat and generations, audit store. |
| `services/admin/` | The admin service: the admin API (`admin_api.rs`), the site (`src/ui/`), Caddy configuration, API keys. Unix only. |
| `services/cpa-supervisor/` | The supervisor in the `askbar-cpa` image: runs CLIProxyAPI, restarts it on a config change, a restart request or a crash, and handles the export and import request files for backups ([ADR 0037](docs/adr/0037-askbar-cpa-image-and-supervisor.md)). |
| `clients/desktop/` | The desktop app, its own Cargo workspace: `src-tauri/` (Cargo package `askbar-client`, program `askbar`), the popup in `ui/` (no build step) and its `tests/`. |
| `deploy/caddy/`, `deploy/cpa/` | Caddy's bootstrap config; notes on CLIProxyAPI. |
| `docs/` | `architecture.md`, `adr/` (decisions), `style/`, `wire-contract.md` (prose), `openapi/` (one file per API), `refactor-log.md`. |
| `scripts/`, `tests/` | Smoke and isolation scripts, the checks, popup tests. |
| `tests/e2e/` | The admin-site browser test, screenshots, a mock LLM. |

This table lists what is on `main`; the target layout is in [ADR 0033](docs/adr/0033-repository-layout.md). The v1 target adds the `auditor`, `model-gateway`, `alerter` and `bootstrap` services ([ADR 0003](docs/adr/0003-v1-service-layout.md)); the controller is gone ([ADR 0005](docs/adr/0005-no-controller-and-self-restart.md)). A new service needs a name and an ADR first ([ADR 0038](docs/adr/0038-names-and-new-components-without-a-name-check.md)).

## Build, run, test

```sh
cargo build --workspace            # server and admin (the app is built in clients/desktop)
cargo fmt --all --check            # CI runs it, here and in clients/desktop
cargo test --workspace             # needs python3 (a mock LLM)
npm install && npm --prefix clients/desktop install && npx playwright install chromium
npm test                           # names and link checks, cargo tests (both workspaces), markdown, UI and admin browser tests, mock LLM
scripts/smoke.sh                   # server + admin + mock LLM, end to end, no Docker
docker compose up -d --build && scripts/check-isolation.sh   # needs Docker; a throwaway stack. ASKBAR_SLOW_CHECKS=1 adds the five-minute alert check
python3 tests/e2e/mock_llm.py 8765     # a fake OpenAI-compatible server to try things without a key
npm --prefix clients/desktop install && npm --prefix clients/desktop run dev   # the desktop app
```

**Third-party notices.** [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lists what ships and its licenses. After you change a dependency (`Cargo.lock`, a lockfile) or an image's contents, run `scripts/third-party-notices.sh` and commit the result; `scripts/third-party-notices.sh --check` says whether it is out of date. It is not part of `scripts/check-docs.sh` or CI, because it needs the crates downloaded, so it cannot run without the network on a fresh checkout. The images section is written by hand in `scripts/third-party-notices.images.md`.

The browser tests start the built `askbar-server` and `askbar-admin`; run `cargo build -p askbar-server -p askbar-admin` first
when you have changed them (the admin site's files are compiled in).

## Adding an endpoint, end to end

1. **Route.** Register it in the right table: `client_routes` in `services/server/src/lib.rs`, `routes()` in
   `services/server/src/server_api.rs`, or `routes()` in `services/admin/src/lib.rs`. Build the path from
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

After 1.0, changes within `/api/client/v1` are additive only (new fields, endpoints, codes); anything else needs a `v2`
beside it for a release. Before 1.0, paths can change to fit [ADR 0022](docs/adr/0022-api-path-conventions.md).

## Database changes

Schema changes append to `MIGRATIONS` in `services/server/src/db.rs` or `services/admin/src/db.rs`; never edit an
earlier entry (its text is what made existing databases). Add a test that builds a database at the previous version,
puts rows in it and opens it with the new code; the existing ones show how. The audit store has its own
migrations in `services/server/src/audit.rs`.

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

- Tests land in the same commit as the code they test. Don't loosen or delete a test unless the contract changed,
  and say so in the commit message.
- The admin site's text goes in through `textContent`, never `innerHTML` (its CSP forbids inline script and style).
- `askbar-server` and `askbar-admin` each have their own copy of the small route-table builder (`routes.rs`), so
  neither crate depends on the other. Keep them alike.
- Secrets never go in `.env`, logs or error messages, and go in backups only under the encryption in
  [ADR 0013](docs/adr/0013-backups.md).
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
- **Review.** The owner reviews every pull request. Changes to the paths that decide who can reach what get extra care.

## Repository settings the owner turns on

These are settings on GitHub, not files, so no pull request can turn them on ([ADR 0017](docs/adr/0017-dependencies-and-patches.md),
[ADR 0018](docs/adr/0018-releases-and-migrations.md)). The workflows and `.github/dependabot.yml` are written for them. The owner
does these in the GitHub web UI; the agent never changes repository settings. Do them in order. Steps A to D can be done now. Step E needs
`images.yml` merged, and its first run on `main`.

### A. Actions permissions

In the repo: **Settings → Actions → General**.

1. **Actions permissions:** *Allow all actions and reusable workflows*. Every action is already pinned by commit hash. For a tighter setting, choose *Allow Thewuuryar, and select non-Thewuuryar, actions* and list `actions/*`, `docker/*`, `dtolnay/*`, `Swatinem/*`, `EmbarkStudios/*`, `aquasecurity/*` and `sigstore/*`.
2. **Approval for fork pull request workflows:** *Require approval for all external contributors*.
3. **Workflow permissions:** *Read repository contents and packages permissions*. Each workflow asks for more in its own `permissions:` block.
4. **Allow GitHub Actions to create and approve pull requests:** leave it unticked.
5. Save.

### B. Environments

**Settings → Environments → New environment.**

1. **Name: `staging`** → *Configure environment*.
   - **Deployment protection rules:** none. No reviewer, no wait timer.
   - **Deployment branches and tags:** choose *Selected branches and tags* → *Add deployment branch or tag rule*.
     - **Ref type:** Tag.
     - **Name pattern:** `v*-rc*`.
     - *Add rule*.
   - **Environment secrets and variables:** none. Publishing uses the workflow's own `GITHUB_TOKEN`, and signing uses OIDC, so nothing is stored.
2. **New environment, name: `production`** → *Configure environment*.
   - **Required reviewers:** tick it and add yourself.
   - **Prevent self-review:** leave it unticked, or you could never approve your own release.
   - **Wait timer:** 0. You may set it to a few minutes, to leave a window for cancelling.
   - **Allow administrators to bypass configured protection rules:** untick it, so every production push waits for your click.
   - **Deployment branches and tags:** *Selected branches and tags* → *Add deployment branch or tag rule*.
     - **Ref type:** Tag.
     - **Name pattern:** `v*.*.*`.
     - *Add rule*.

     GitHub's patterns can't exclude an rc tag. The workflow sends `-rc` tags to `staging` itself, and step D means only you can create tags.
3. **Save protection rules.**

A tag push then shows *Waiting* under **Actions**. Open the run → *Review deployments* → tick `production` → *Approve and deploy*.

### C. Ruleset for `main`

**Settings → Rules → Rulesets → New ruleset → New branch ruleset.**

1. **Ruleset name:** `main`. **Enforcement status:** Active.
2. **Bypass list:** leave it empty. An emergency fix still goes through a pull request (ADR 0001 rule 5 lets its ADR follow later).
3. **Target branches:** *Add target* → *Include default branch*.
4. **Rules:**
   - **Restrict deletions:** on.
   - **Require linear history:** on. Merges are squash or rebase only (AGENTS.md).
   - **Require a pull request before merging:** on.
     - **Required approvals:** 0. You can't approve your own pull request, and your merge is the approval.
     - **Require conversation resolution before merging:** on.
     - **Allowed merge methods:** Squash and Rebase.
   - **Require status checks to pass:** on.
     - **Require branches to be up to date before merging:** on.
     - *Add checks*: `lint`, `test-services`, `test-desktop`, `isolation`, `dependencies`, `images`, `cpa-image` and `secrets`. A check appears in the picker only after it has run once on a pull request; all of these already have. Pick the ones from the GitHub Actions source.
   - **Block force pushes:** on.
5. **Create.**

### D. Ruleset for release tags

**Rulesets → New ruleset → New tag ruleset.**

1. **Name:** `release tags`. **Enforcement:** Active.
2. **Bypass list:** *Add bypass* → *Repository admin* → mode *Always*. That is you. Nothing else is listed, so neither an agent nor the Claude GitHub App can create, move or delete a release tag.
3. **Target tags:** *Add target* → *Include by pattern* → `v*`.
4. **Rules:** *Restrict creations*, *Restrict updates*, *Restrict deletions* and *Block force pushes*, all on.
5. **Create.**

### E. Packages

After `images.yml`'s first run on `main` has pushed the edge images.

1. Go to `https://github.com/Thewuuryar?tab=packages`. There is one package for each image: `askbar-bootstrap`, `-server`, `-admin`, `-auditor`, `-model-gateway`, `-alerter`, `-caddy` and `-cpa`.
2. For each one: open it → **Package settings**.
   - **Manage Actions access:** `Thewuuryar/askbar` is listed with the *Write* role. It is added for you when the repo's workflow creates the package; if it is missing, *Add Repository*.
   - **Inherit access from source repository:** on.
   - **Danger Zone → Change visibility → Public.** Type the package name to confirm.

     A public package can't be made private again. Do this only once you've checked the first images.
3. Check that a pull works without signing in, from any machine:

   ```sh
   docker logout ghcr.io && docker pull ghcr.io/thewuuryar/askbar-server:main
   ```

### F. Security

**Settings → Security** (the page may be named *Code security* or *Advanced Security*). Turn on:

- **Dependabot alerts** and **Dependabot security updates**. `dependabot.yml` expects both.
- **Secret scanning** and **Push protection**. Both are free for public repositories.
- **Private vulnerability reporting**, so reports can follow `SECURITY.md` without a public issue.

### G. Notifications for the CVE alert (D30)

This is your account, not the repo.

Go to **github.com/settings/notifications → System → Actions**.

- Tick **Email**.
- Tick **Only notify for failed workflows**.

The weekly scheduled `ci` run then emails you when a new advisory makes it fail. It is the CVE alert; there is no other. GitHub stops scheduled runs in a repository with no activity for 60 days, so push something now and then.

**The Renovate app** on the repository (`renovate.json`) proposes the version updates and keeps Actions and images pinned.

## Cutting a release candidate and a release

Only the owner pushes a release tag: the tag ruleset (step D) lets nobody else, and `images.yml` refuses a tag whose commit is not on `main`. The workflow never publishes a release; it drafts one.

1. **A release candidate.** On the `main` commit to release, with its `ci` run and its edge `images` run green: `git tag v1.0.0-rc.1 <commit> && git push origin v1.0.0-rc.1`. The `staging` environment has no reviewer. The run builds and signs the images, builds the installers, and drafts a **prerelease** holding `compose.release.yaml`, the SBOMs, the installers and their signatures. Check the draft, and the signatures as the guide says ([Verifying images and files](docs/guide.md#verifying-images-and-files)).
2. **Staging.** Run the candidate as the guide says ([Staging a release](docs/guide.md#staging-a-release)) and complete the checklist in `docs/releases/<version>.md`. A fix gets its own pull request, then a new candidate (`-rc.2`); never move a tag.
3. **The release.** Push `v1.0.0` on the commit that passed staging. The `production` environment waits: **Actions** → the run → *Review deployments* → tick `production` → *Approve and deploy*. The run drafts the release (with `docs/releases/1.0.0.md` as its notes); read it, then publish the draft by hand.

An ignore for a vulnerable image package goes in `.trivyignore.yaml`, one with its ID, a statement and an expiry date
(`scripts/check-docs.sh` refuses one without them); an ignore for a vulnerable crate goes in `deny.toml`, with its ID, a reason and a
date to look again. `scripts/check-deny-fixture.sh` shows that `cargo deny` still fails on a crate pinned to a known-vulnerable release.

## Before you open a pull request

Run `npm test` and `scripts/smoke.sh`. If you touched `compose.yaml`, `Dockerfile`s, Caddy or anything about who can
reach what, run `scripts/check-isolation.sh` too (it needs Docker) and say that you did.
