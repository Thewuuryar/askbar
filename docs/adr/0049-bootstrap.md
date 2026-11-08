# 0049. The `bootstrap` component

Status: Proposed
Date: 2026-11-08
Kind: component
Source: plan step 2.1 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

[0004](0004-bootstrap-and-per-link-tokens.md) decides that a `bootstrap` container creates per-link tokens, and [0005](0005-no-controller-and-self-restart.md) decides that rotation is a host command. The container does not exist yet: today the admin makes the one token it has, the admin-to-server token. This record describes the component plan step 2.1 builds, and it is proposed before that step so the owner accepts it first. The sources are 0004, 0005, [0032](0032-container-hardening.md) and the Links and Volumes tables in [architecture.md](../architecture.md); this record links them instead of repeating them.

## Decision

- **What it is:** `services/bootstrap`, a Dockerfile target and image `askbar-bootstrap`. It creates the per-link tokens and exits.
- **No network.** It has no network and publishes no port. It makes and accepts no connection, and no other service calls it.
- **What it holds:** nothing of its own. It mounts credential volumes only, and no data volume. It keeps no state between runs: the tokens and hashes it writes belong to the services that read them ([architecture.md](../architecture.md), Volumes).
- **What it does:**
  - for each link in the architecture Links table that uses a per-link token or an append token, it generates the token, writes it to the caller's credential volume, and writes the token's SHA-256 to the receiver's credential volume;
  - each file gets the owner uid and mode that its reader needs. Every container runs `cap_drop: [ALL]` ([0032](0032-container-hardening.md)), so root cannot read or write another uid's files, and `bootstrap` has to create each file as its reader needs it;
  - the receiver maps each token to one caller and its allowed routes ([0004](0004-bootstrap-and-per-link-tokens.md)).
- **Run once:** it runs before the services that use its tokens and exits. Those services wait for it with `depends_on: condition: service_completed_successfully`. Run again on a stack that already has tokens, it changes none of them.
- **Rotation:** from the host, `docker compose run --rm bootstrap rotate [link]` writes new tokens and hashes for one link or all links. The admin then restarts the affected services so they read them ([0005](0005-no-controller-and-self-restart.md)). The admin cannot rotate tokens itself.
- **Replaces:** the token the admin makes today for the server API. The `server-token` volume keeps its name ([architecture.md](../architecture.md): data volumes are never renamed).
- **Volumes:** credential volumes only: each caller's and each receiver's token volume. Which volumes, and their names, come from the Links table when step 2.1 is built; there is a unit test of the link list.
- **Networks:** none.
- **Links:** none. It writes files and talks to no service.
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with no exception stated here; `check-isolation.sh` lists `bootstrap` in its hardening section.

## Alternatives considered

- **The admin keeps making tokens:** rejected by [0004](0004-bootstrap-and-per-link-tokens.md). The admin's downtime would be able to expire a service credential.
- **A private CA with mutual TLS, or first-contact claims:** rejected by 0004.
- **Compose file secrets:** rejected by 0004, because those files must exist before startup.

## Consequences

Setup needs no manual wiring. A stolen token works until it is rotated ([0016](0016-risk-register.md)). The isolation script has one more service to check, and a missing or wrongly owned token file shows up as a service that fails to start.

**Open questions for the owner.** The sources do not settle these, so this record decides none of them; plan step 2.1 waits for the answers.

1. **Which uid does `bootstrap` run as?** With `cap_drop: [ALL]` a single uid cannot write files owned by several other uids. Options the plan names are "create each file with the owner and mode its reader needs" or "run as that reader's uid". The first needs `CAP_CHOWN` (an exception to 0032); the second needs one run per reader.
2. **Which links get a token in the first PR?** Only the admin-to-server link exists today. The others (auditor, `model-gateway`, `alerter`) belong to later steps. Does 2.1 write only the links whose receivers exist, or all links in the table with the receivers added later?
3. **The `cpa` API key.** The Links table lists the `model-gateway` to `cpa` link as a `cpa` API key, not a per-link token. Does `bootstrap` generate it?
4. **Token format and size.** Not recorded in the sources.
5. **Credential volume names and layout.** One volume per link, or one per service? New volumes need names that are never changed afterwards.
6. **Does `rotate` accept a link name only, or also a caller?** The sources give `rotate [link]` and no naming for links.
7. **A hardening exception.** If the answer to question 1 needs a capability, that is an exception to 0032 and needs the owner's approval.

## Amendments

None yet.
