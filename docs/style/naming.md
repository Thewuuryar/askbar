# Naming

Everything here is how the code already behaves on `main`, except items marked **(proposed)**.

## The words

Use **client**, **server**, **admin** and **controller**, in paths, variables, UI text and docs. Their meanings, and the other terms (user, provider, account, token and key), are in the [glossary](../architecture.md#glossary). The old words are refused by `scripts/check-names.sh`. A line that must use one (a migration, an old path in a test) carries `names-ok` and a reason; whole files are listed in `scripts/names-allow.txt`.

## Code

| What | Rule | Example |
| --- | --- | --- |
| Cargo packages | `askbar-<role>` | `askbar-server`, `askbar-proto` |
| Desktop package and program | package `askbar-client`, program `askbar` | `src-tauri/` |
| Rust | the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/naming.html) | `snake_case` items, `CamelCase` types |
| Rust files | `snake_case.rs` | `client_api.rs`, `server_api.rs` |
| JavaScript (`ui/`, admin site) | `camelCase` functions, no build step | `showStatus`, `copyText` |
| Tests | in the crate's `tests/`, named for the area | `askbar-server/tests/streaming.rs` |

## Wire and storage

| What | Rule | Example |
| --- | --- | --- |
| URL path | `/<audience>/<...>/v<N>/<resource>`; plural nouns; built from `askbar_proto::paths`, never a new literal | `/api/client/v1/pairings` |
| Action route | only for something that is not a plain create, update or delete | `POST .../cancel` |
| JSON fields | `snake_case` | `min_client`, `client_request_id` |
| Error codes | `snake_case`, in `askbar-proto/src/problem.rs`, only ever added | `budget_exceeded` |
| Error type URN | `urn:askbar:problem:<code>` | |
| Ids | ULID, made by the server, opaque to clients | |
| Timestamps | UTC unix seconds, `created_at`, `updated_at` | |
| Headers | `X-Askbar-<Thing>` | `X-Askbar-Client-Version`, `X-Askbar-Admin` |
| Tokens and strings | fixed prefix | `abt_` (client token), `askbar-pair:` |
| OpenAPI files | `docs/openapi/<audience>.v<N>.yaml` | `client.v1.yaml` |

## Configuration and containers

| What | Rule | Example |
| --- | --- | --- |
| Environment variables | `ASKBAR_<COMPONENT>_<THING>` with a kind suffix: `_ADDR`, `_URL`, `_DIR`, `_FILE`, `_SOCKET` | `ASKBAR_SERVER_API_ADDR`, `ASKBAR_ADMIN_SOCKET` |
| Compose services | the role, lower case | `server`, `admin`, `caddy`, `cpa`, `controller` |
| Containers and images | `askbar-<service>` | `askbar-server` |
| Networks | what they connect | `edge`, `control`, `cpa-admin`, `server-api` |
| Volumes | kebab-case; **never renamed** | `askbar-secrets`, `audit-data` |

## Git **(proposed)**

Branches and commits: see "Branches and commits" in [AGENTS.md](../../AGENTS.md).
