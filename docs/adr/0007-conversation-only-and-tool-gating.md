# 0007. Conversation only, and what gates tools

Status: Accepted (2026-11-03): conversation only through 1.0. Where the executor runs and which tools come first are left for the ADR that proposes them.
Date: 2026-11-03

## Context

The README says Askbar is conversation only: text in, text out, no tools, no shell, no file access. The goals document lists, under "Later": web research through an executor (SearXNG and plain HTML fetch), code execution in its own container (on the NAS or other hardware), and audit coverage of tool calls, commands, network requests and file diffs. It also asks that running agents can never change settings ([0003](0003-admin-and-server-isolation.md)).

Both cannot stay true without a rule for when and how tools arrive.

## Decision

Askbar stays conversation only through 1.0. A change that gives the model any tool needs its own ADR first, and when tools are added these invariants hold from the first commit:

1. Tools run in a new container (the **executor**) that has no route to the admin, the server API, the controller or any secret volume. Name it as a fifth word in [architecture.md](../architecture.md) before it exists.
2. Network egress from the executor is denied by default and allowed per destination.
3. Every tool call, command, network request and file change is an audit span written **before** it runs. If the audit write fails, the call does not run.
4. Tool output is untrusted text. It can never change settings, create a client, or reach the admin API.
5. Per-user token limits and the kill switch cover tool use.
6. The popup window still gets no filesystem or network permission.

## Alternatives considered

- **Stay conversation only for good.** Simplest, and keeps the audit story small. Loses the research and code features.
- **Tools inside `askbar-server`.** Rejected here: it puts untrusted execution in the container that holds provider keys and the audit store.

## Left for the ADR that proposes tools

1. Where the executor runs: same host, or a separate machine (the NAS), and how it is reached and authenticated.
2. Which tools first: web research (fetch and search) or code execution.

## Consequences

Until then no tool, shell or file access reaches the model, which keeps the audit trace to replies only. When tools are proposed, `scripts/check-isolation.sh` gains a check for each invariant above.
