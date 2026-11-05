## What and why

<!-- One or two sentences. Link the issue or plan if there is one. -->

## Verified

- [ ] `npm test`
- [ ] `scripts/smoke.sh`
- [ ] `scripts/check-isolation.sh` (required if `compose.yaml`, a `Dockerfile`, Caddy, a network, a volume or who-can-reach-what changed)

## Not verified

<!-- Say what you did not run or could not check (for example: no Docker here, Windows and macOS untested). "Nothing" is a fine answer. -->

## Contract and boundaries

- [ ] No change to an API, a migration, an error code, a volume, a network or a published port
- [ ] If there is one: OpenAPI file, `docs/wire-contract.md`, migration test and `docs/architecture.md` updated in this PR
- [ ] If a new library was added: the reason, under "What and why"
- [ ] If a boundary moved, a container, image or service was added, or a dependency is outside the license allowlist: an ADR in `docs/adr/`
- [ ] No secret, key, token or message content in logs, errors, tests or fixtures

## Notes for the reviewer

- [ ] Written or changed with an AI agent (name it)
- [ ] A test was loosened or removed, and the commit message says why
