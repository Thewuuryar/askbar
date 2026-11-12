# 0045. The `askbar-proto` crate

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

The server, the admin and the desktop client must agree on every type and path they exchange, and an error code must mean the same thing on both ends. The crate existed before the rule that each component has its own ADR; its name was kept on purpose ([architecture.md](../architecture.md), "Considered and not changed"). This record describes it as it is on `main` after plan step 1.11. It follows [0022](0022-api-path-conventions.md), [0023](0023-api-error-format.md), [0033](0033-repository-layout.md) and [0015](0015-certificates.md).

## Decision

- **What it is:** `crates/askbar-proto`, a library crate in the root workspace, also built into the desktop client's workspace by path ([0033](0033-repository-layout.md)). It is not a container, holds no data, opens no port and does no I/O.
- **What it holds:**
  - the wire types of the client API (conversations, messages, spaces, the chat request and its events, pairing) and of the server API (the configuration snapshot the admin pushes);
  - **from plan step 4.1,** the alerter's types: `paths::ALERTER_V1`, and `alerter` (`Event`, `Kind` with its severity and the link allowed to send it, `AlerterConfig`, `AlerterStatus`), whose typed fields are the only content an event has ([0055](0055-alerter.md));
  - **from plan step 5.1,** the policy type (the privacy notice, its version and the AI warning) in the snapshot and the settings, with `serde(default)`, so `MIN_CLIENT` does not change;
  - **from plan steps 6.2 and 6.3,** the auditor's backup and restore types (the backup key, the export, restore and its errors);
  - `paths`: every API prefix, `/api/{name}/v{major}` ([0022](0022-api-path-conventions.md));
  - `problem`: the problem-details shape and `ErrorCode`, the registry of error codes ([0023](0023-api-error-format.md));
  - `sse`: the incremental Server-Sent Events parser the server uses for provider streams and the client for the chat stream;
  - the pairing key's encoding, including the certificate pin ([0015](0015-certificates.md)), and the client version header and its parser.
- **Dependencies:** `serde`, `serde_json` and `base64`. `axum` only behind the `axum` feature, which the server and the admin turn on for their `ApiError` response type; the desktop client builds without it.
- **Users:** the server, the admin, the auditor, `model-gateway`, the alerter, `askbar-service` ([0036](0036-askbar-service-crate.md)) and the desktop client.
- **Rule:** within `/api/client/v1` after 1.0, a field, type or error code is only added, never renamed or removed (AGENTS.md).

## Alternatives considered

Not recorded: the crate predates this record.

## Consequences

A change to a shared type changes every user at once, and the OpenAPI tests in `services/server` and `services/admin` fail if a route, its file or the error registry drift apart. The crate's own tests cover the paths, the problem codes, the SSE parser and the pairing key.

**Where the code and an accepted ADR disagree:** none found. One observation for review: the desktop client compiles the server API's snapshot types too, since they live in the same crate. They hold no secret, but a later split could keep the client's build to the client API.

## Amendments

None yet.
- 2026-11-11: design for plan steps 4.1, 5.1, 6.2 and 6.3 (#70): `ALERTER_V1` and the alerter types, the policy type, the auditor's backup and restore types, and the alerter among its users. The code PRs add the 'built' lines.
- 2026-11-11: built `paths::ALERTER_V1` and the alerter types (plan step 4.1, PR S, #73): `alerter::{Kind, Severity, Sender, Event, Fields, AlerterConfig, NtfyConfig, AlerterStatus, TestResult}`, with `Kind::ALL`, each kind's severity, senders and fields, and `Event::check`. Every type refuses a field it does not know, and `NtfyConfig`'s `Debug` shows no server, topic or token. The audit crate's `AlertDeliveryStart` and `AlertDeliveryRecord` came with PR T. The policy type and the backup and restore types are not built yet.
- 2026-11-12: `Snapshot.backup_public_key` and `AlerterConfig.backup_public_key` (plan step 6.1, PR AI, #86): the backup public key, `Option<String>`, additive (`serde(default)`; absent while none is set).
