# 0043. The `mock-llm` component

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

Askbar's tests and the Mac check need a model provider that answers fast, costs nothing, needs no key and can be made to fail on purpose. The mock existed before the rule that each component has its own ADR. This record describes it as it is on `main` after plan step 1.11. It follows [0018](0018-releases-and-migrations.md) (staging uses the existing mock model) and [0032](0032-container-hardening.md).

## Decision

- **What it is:** `tests/e2e/mock_llm.py`, one Python file with no dependencies beyond the standard library. It answers the OpenAI-compatible `/v1/models` and streaming `/v1/chat/completions`, and Anthropic's `/v1/messages`, with a fixed Markdown reply. Words in the message or the model name choose a behaviour: `slow`, `fail` (401), `429`, `429once`, `500`, `reset`, `hang`, `drop`, and `cached` (usage with cached tokens). `tests/e2e/mock_llm_test.py` tests it.
- **Test use, not in the stack:** the Rust tests of the server, the admin and the desktop client, `tests/e2e/*.js` and `scripts/smoke.sh` start it as a local process on a free port of `127.0.0.1`.
- **Dev profile:** `compose.yaml` runs it as the `mock-llm` service only under `--profile dev`, for the Mac check. The image is `python:3.12-slim`; the script is bind-mounted read-only from the repository; it listens on `0.0.0.0:8765` inside the container (`MOCK_LLM_HOST`), on the `edge` network, with no published port. Providers reach it at `http://mock-llm:8765/v1`.
- **What it holds and reaches:** nothing. It keeps only an in-memory set of the `429once` messages it has seen, has no volume of its own, calls nothing and receives no secret (any key is accepted).
- **Hardening:** the shared defaults of [0032](0032-container-hardening.md), with no exception; memory limit 128 MB.
- **Never shipped:** it is not part of a release and is never in the default profile.

## Alternatives considered

Not recorded: the mock predates this record.

## Consequences

Tests and the Mac check run without a provider account or network access. A provider behaviour that tests need is added to the mock, with a test in `mock_llm_test.py`.

**Where the code and an accepted ADR disagree:**

- [0017](0017-dependencies-and-patches.md) (pin images to digests): the dev profile uses `python:3.12-slim` by tag.
- [0003](0003-v1-service-layout.md) (no host bind mounts): the dev service bind-mounts the script from the repository. It is dev-only and read-only.

## Amendments

None yet.
