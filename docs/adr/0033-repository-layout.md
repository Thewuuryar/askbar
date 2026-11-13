# 0033. Repository layout

Status: Accepted
Date: 2026-11-06
Source: owner request with the v1 plan ([plan-v1.md](https://github.com/Thewuuryar/askbar/blob/504481bc62e9290bb20be6401dfcf379a3bdedb6/docs/plan-v1.md))

## Context

V1 adds four services (`auditor`, `model-gateway`, `alerter`, `bootstrap`) and two shared crates to the three Rust packages at the top of the repository. The desktop client shares the root Cargo workspace with the services, so the Dockerfile rewrites `Cargo.toml` to leave it out, and the GTK and WebKit libraries it needs are in the way of a plain `cargo test` on a server-only machine. Caddy is built from a second Dockerfile.

## Decision

```
askbar/
  Cargo.toml  Cargo.lock      services and crates only
  compose.yaml                v1 stack; cpa under a profile
  Dockerfile                  one target per service
  crates/                     askbar-proto/, askbar-audit/, askbar-service/
  services/                   server/, admin/, auditor/, model-gateway/, alerter/, bootstrap/
  clients/
    desktop/                  own Cargo workspace and package.json: src-tauri/, ui/, tests/
    ios/                      Swift, generated from docs/openapi/client.v1.yaml
  deploy/                     caddy/, cpa/
  tests/e2e/                  mock_llm.py, admin.test.js, the 3-user load target
  scripts/
  docs/                       adr/, openapi/, style/
```

- A directory under `services/` builds one image, as one target of the root `Dockerfile`, named for its glossary component ([ADR 0021](0021-names-and-new-components.md)). A shared library goes under `crates/`.
- Cargo package and binary names don't change with the directory: `askbar-server` lives in `services/server/`.
- The root workspace holds services and crates only. `clients/desktop/` is its own Cargo workspace with its own lockfile, and depends on `crates/askbar-proto` by path.
- `src-tauri/` and `ui/` keep their names, under `clients/desktop/` ([architecture.md](../architecture.md#considered-and-not-changed)).
- `askbar-audit` and `askbar-service` are created by the first change that uses them, and `clients/ios/` by the iOS work, not as empty directories.

## Alternatives considered

- **Keep every package at the top level:** rejected. With six services, two crates and two clients the top level stops showing what is a service.
- **One workspace for the client and the services:** rejected. It ties the client's dependencies and system libraries to every server build.

## Consequences

Paths in scripts, tests, compose and the docs change once. A server-only machine builds and tests the root workspace without GTK. The client gets its own `Cargo.lock`, so shared dependencies can drift between the two workspaces. Enforced by the build: compose, the Dockerfile and the tests use these paths.

## Amendments
