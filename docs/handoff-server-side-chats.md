# Hand-off: server-side chats and audit (0.4.0)

Branch `claude/askbar-server-side-chats-26xdjw`. Nothing was pushed and no pull request was opened. 116 commits, one per step in the plan's order (7.12 has two: a first fix and the bars).
This file is not committed.

## What changed, by phase

- **0, mock model.** `tests/mock_llm.py` gained modes `429`, `429once`, `500`, `hang`, `drop`, `fail`, `slow`, `cached`, `reset` (also chosen by the same words in the model name), an OpenAI usage chunk and an Anthropic `/v1/messages` endpoint. Its self-test runs in `npm test`.
- **1, detached generations (1.1-1.25).** Replies are server tasks: they survive a disconnect, stream with SSE ids that count deltas, resume at `GET /api/v1/generations/{id}/stream?after=N`, cancel with `POST .../cancel`, and show up as `pending` in the conversation detail and in `GET /api/v1/generations?status=running`. `client_request_id` makes a send idempotent. A queue of 5 per conversation (the sixth gets 409). Restart safety: `sweep_on_start` and a SIGTERM `shutdown` end running replies as `interrupted` with their partial text. The client resumes quietly with backoff 1, 2, 4, 8, 16, 30 s, Stop calls cancel, other windows can attach, and an outbox keeps unsent messages. Version 0.4.0, `MIN_CLIENT` 0.4.0.
- **2, timeout.** `generation_timeout_secs` (default 300, range 30-3600) travels in the snapshot, is validated by the server, set in the admin Settings tab, stamped as `deadline_at` and enforced. The client keeps partial text on an error.
- **3, usage.** Usage is requested from OpenAI-style providers (`stream_options.include_usage`), parsed for OpenAI and Anthropic, estimated (characters / 4) when absent, and recorded per generation.
- **4, audit store.** `audit.db` (own volume `audit-data`, mounted only by the server): spans (`turn`, `gate`, `attempt`, `tool`, `agent_step`) and payloads, triggers (nothing younger than 7 days is deleted; a span changes only while `running`), a bounded writer thread, refusal on backlog (503 with `Retry-After`), start-up sweep, daily retention, admin proxy endpoints.
- **5, sealing.** The audit passphrase (at least 20 characters) goes through Argon2id (64 MiB, 3 passes, 1 lane) to an X25519 key; the server holds only the public key and seals content with sealed boxes. Setup screen, keyring (`audit_keys`), access log (`audit_access`), rotation, an open endpoint with a rate limit. No key: 503 `audit_key_missing`.
- **6, router, gates, cost.** Candidate order, failure classification, up to 2 retries (250 ms then 1 s, or `Retry-After`), fallback, no switch after the first delta, a `provider` SSE event, bad-credential flag, kill switch (setting and behavior for running replies), token-budget gate (daily and monthly, UTC), cost computed at read time, Anthropic prompt-cache markers, billing and prices passed by the admin.
- **7, Audit tab.** Overview, requests, trace waterfall, routing, content (passphrase dialog), access history, settings; all server text enters through `textContent`. Browser tests and a screenshot script.
- **8, client follow-ups and docs.** The client maps `queue_full`, `budget_exceeded`, `kill_switch`, `audit_key_missing` and `busy` (one retry after `Retry-After`), shows "Answered by ..." when another provider served, shows timed-out replies as "Stopped after N ..." with **Try again**, and has an end-to-end reconnect test (drop and resume, then a server restart). The CLIProxyAPI image is pinned to `v8.0.51@sha256:15d13889e1a39a688be306dfbf2f6cf9003e99cd913b5070fa463f8ab2aa0031`. README, wire contract and refactor log updated. The admin backup was checked: it has no audit tables, and the README says how to copy the `audit-data` volume.

## Release gate (8.11)

Outputs are in `/tmp/claude-0/-home-user-askbar/daac6cf8-503b-57c7-928c-03b0bcb4e537/scratchpad/`.

| Run | Result |
| --- | --- |
| `cargo test --workspace` (`release-1-cargo-test.txt`) | 363 passed, 0 failed, 1 ignored (the live-server client e2e that needs a keychain) |
| `npm test` (`release-2-npm-test.txt`) | passed: markdown 19, UI, admin, mock self-test (12) |
| `scripts/smoke.sh` (`release-3-smoke.txt`) | 13 passed, 0 failed, 1 skipped (needs a person at a desktop) |
| `scripts/check-isolation.sh` (`release-4-isolation.txt`) | **not run**: no Docker daemon here (exit 2, "failed to connect to the docker API") |

CI was not triggered. 8.10 (version cleanup) made no commit: every remaining `0.3.0` is history (the changelog, README verification notes, `.env.example` comment) or deliberate (`version_gate.rs` sends 0.3.0 and expects 426).

## Not done, and why

- **Isolation checks.** `scripts/check-isolation.sh` needs Docker. The checks added in 4.15 and 5.14 (audit store not readable from the admin or Caddy, the server holds no private key or passphrase) have never been run. Run it against a throwaway stack: `docker compose up -d && scripts/check-isolation.sh`, at the phase 4, 5 and 8 checkpoints' scope, then once more as the release gate.
- **CLIProxyAPI usage passthrough** (8.7) is not verified; see `docs/refactor-log.md` for what the server does without it and how to check.
- **The image was pinned from the registry's tag list** and not pulled or started.
- **README "Verified, and not"** still describes 0.3.0 verification; it was not rewritten because the plan lists no step for it.
- The plan's per-step file lists were not re-read after my context was compacted partway through phase 8. Where I touched files a step did not obviously list, the commit body says so (8.3: `ui/main.js`, `ui/style.css`, `tests/mock-tauri.js`, `tests/ui.test.js`; 8.4: the same files). Earlier out-of-list files are named in their own commit bodies (for example `src-tauri/Cargo.toml` for `uuid`, `scripts/smoke.sh`, `testing.rs`, the mock).

## Interpretation notes (please check)

1. **Budget gate counts tokens, not cost.** The plan says the gate sums "cost", but the budgets are token limits, so it sums tokens for the UTC day and month. Subscription providers count 0. Cost is derived at read time for display.
2. **Timed-out replies are recognised by their words.** The wire has no code for them, only `error` with "Timed out after N unit"; the client turns that into kind `timed_out`. Documented in the wire contract.
3. **Kill switch UI.** The admin Settings tab has a kill switch and a second box for whether it also stops running replies; the plan implied the setting but not the controls.
4. **Millisecond times** for spans are in `attrs.started_ms` / `attrs.ended_ms` (merged with `json_patch`), not columns.
5. **Existing tests changed because their contract changed** (each named in its commit body): 1.5 disconnect test; 1.24 `version_gate`; 2.x setup-status assertion in `admin_setup.rs`; 5.3, 5.12 and 6.16 `user_version` assertions in `service.rs`; 6.7 the 1.1 upgrade test; 6.9 `chat.rs` event order; 6.10 gate span lists; 6.15 Anthropic system-prompt shape; 7.10 `caddy_apply.rs` master-key text; 8.2 `StreamEvent::Error` patterns (new field); 8.4 the timeout assertions in `remote.rs` and `ui.test.js`.
6. The wire contract's Errors section had a dangling clause from an earlier edit and said errors carry no body; fixed in 8.8.

## Manual checks left for a person

- Close and reopen the app mid-reply: the reply is still there and complete.
- iPhone (or any second device) in airplane mode mid-reply, then back online.
- Send 6 messages in one conversation: the sixth is refused and its text kept.
- Set the timeout to 30 s and ask for something long: "Stopped after 30 seconds", partial text kept, Try again works.
- Kill switch with each setting of "also stop running replies".
- Audit tab with real CLIProxyAPI: tokens real or estimated (`usage_estimated`), content opens with the passphrase and appears in the access history, a wrong passphrase is logged.
- Run `scripts/check-isolation.sh` with Docker (above).

## Commits

```
39af2f2 0.2: mock LLM modes 429 and 500
fffd2f4 0.3: mock LLM modes hang and drop
23c318b 0.4: mock LLM OpenAI usage chunk
93177a9 0.5: mock LLM Anthropic streaming endpoint
6f3e81c 0.6: run the mock self-test in npm test
b2ca808 1.1: migration v4 adds generations
ea653f9 1.2: generation queries
36a1667 1.3: generation registry (not wired yet)
b5a94f3 1.4: test that a reply keeps generating after disconnect (ignored)
3d55cf9 1.5: detach the generation task from the client
781f8e7 1.6: persist generation progress
d8e3b7c 1.7: wire shape: generation_id and SSE ids
35ebd85 1.8: stream endpoint for resuming a generation
aacc9c6 1.9: cancel endpoint
9758b4e 1.10: pending views
489101e 1.11: idempotent start with client_request_id
891d54f 1.12: queue admission
a79cc34 1.13: queue runner and queued cancel
026070e 1.14: conversation delete cancels its generations
892d29d 1.15: restart safety
167bd6e 1.16: prune old generation rows
2915567 1.17: client parses generation_id and queued
a953051 1.18: resume a dropped reply stream
40c3216 1.19: Stop calls the cancel endpoint
1f323e5 1.20: attach to running replies; waiting messages in the UI
76824f4 1.21: outbox store
df81bdc 1.22: reconcile the outbox on reconnect
d1ac581 1.23: outbox UI
5dd359e 1.24: version 0.4.0
53c883a 1.25: contract docs for server-side generations
82c071d 2.1: snapshot field generation_timeout_secs
b5c87b9 2.2: validate generation_timeout_secs in the server
e62cfb3 2.3: admin builds generation_timeout_secs
26c61a6 2.4: admin API for the generation timeout
69d3585 2.5: test-only generation timeout override
88efb4e 2.6: write the deadline when a generation starts
67b657b 2.7: enforce the generation deadline
86609ae 2.8: Generation timeout field in the admin Settings tab
eaa0acf 2.9: client keeps partial text on an error
e8e98a1 3.1: usage and report types
57655e0 3.2: timing and report
ced875b 3.3: ask for usage in OpenAI-style requests
d506931 3.4: parse OpenAI usage
bd511b0 3.5: parse Anthropic usage
42b3061 3.6: estimate usage when the provider reports none
4fb9270 3.7: record usage
61db595 4.1: audit tests (ignored)
316a213 4.2: audit.db schema
5b9d888 4.3: audit.db triggers
7f7f6ff 4.4: audit writer task
e9dad2a 4.5: audit recorder in AppState
1fe8b83 4.6: audit span helpers
6906e8b 4.7: turn spans
48a9203 4.8: attempt spans
ad1d205 4.9: queue wait span
9b34fa5 4.10: refuse new generations on audit backlog
84298a0 4.11: sweep running spans at start-up
b6167f4 4.12: audit retention job
a4c7c3d 4.13: audit management endpoints
aca1507 4.14: audit volume
fae3d6b 4.15: isolation checks cover the audit store
32dedb5 4.16: document audit endpoints
7cb1442 5.1: sealing tests (ignored)
0b6e802 5.2: derive the audit key from a passphrase
d1ae1b0 5.3: audit keyring table
8fdca3c 5.4: setup endpoint for the audit key
a36db6e 5.5: setup screen for the audit key
a549b86 5.6: the public key travels in the snapshot
6cf062c 5.7: seal module
b5f641b 5.8: refuse generations without an audit key
c55c841 5.9: sealed payload capture
23cff03 5.10: cap payload size
cb980a0 5.11: ciphertext endpoint
798cfef 5.12: the admin opens sealed content, with an access record
d43eb12 5.13: audit key rotation
7c7c4fa 5.14: isolation covers key handling
4fad883 5.15: document sealed content
04e46cb 6.1: router tests (ignored)
314a22a 6.2: provider billing and price fields
9b08159 6.3: router candidate order
e1ab511 6.4: failure classification
e456b07 6.5: retries with backoff
8bcff16 6.6: fallback between providers
0ae7ed0 6.7: flag providers with bad credentials
0e47cca 6.8: generations use the router
c7a814c 6.9: provider event
fb41dee 6.10: kill switch gate
3650f7d 6.11: kill switch behavior setting
517c83a 6.12: budget gate
cc41504 6.13: cost calculation
9523f98 6.14: cost in the audit endpoints
cefbd2c 6.15: prompt cache markers
50c1298 6.16: admin passes provider billing and prices
c6fb294 6.17: document the router and gates
c739c9c 7.1: audit tab tests (pending)
cace1af 7.2: audit proxy routes
67bf197 7.3: audit tab shell
66959f1 7.4: audit overview
8cadbdf 7.5: audit requests table
5d3e00b 7.6: trace waterfall
128c8e6 7.7: routing view
5436ebe 7.8: content view
c7cb32d 7.9: content access history
c3ed478 7.10: audit settings
e4714f0 7.11: audit tab tests enabled
8ac49d7 7.12: audit tab screenshot script (and a first fix)
4e2f6f4 7.12: draw the overview bars
cafa349 8.1: client error mapping tests (ignored)
1a8b5cf 8.2: client maps new server errors
f1596f8 8.3: client shows the serving provider
1b578bb 8.4: client shows timed-out replies
20bda26 8.5: end-to-end reconnect test
d38a4bd 8.6: pin the CLIProxyAPI image
23588b6 8.7: record CLIProxyAPI usage behavior
05931c3 8.8: README and docs
5f67b3e 8.9: backup leaves out audit.db
```
