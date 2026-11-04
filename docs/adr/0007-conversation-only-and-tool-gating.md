# 0007. Conversation only, and what gates tools

Status: Proposed
Date: 2026-11-03

## Context

The README says Askbar is conversation only: text in, text out, no tools, no shell, no file access. The goals document lists, under "Later": web research through an executor (SearXNG and plain HTML fetch), code execution in its own container (on the NAS or other hardware), and audit coverage of tool calls, commands, network requests and file diffs. It also asks that running agents can never change settings ([0003](0003-admin-and-server-isolation.md)).

Both cannot stay true without a rule for when and how tools arrive.

## Decision (proposed)

Askbar stays conversation only until this ADR is accepted with an answer to the questions below. When tools are added, these invariants hold from the first commit:

1. Tools run in a new container (the **executor**) that has no route to the admin, the server API, the controller or any secret volume. Name it as a fifth word in [architecture.md](../architecture.md) before it exists.
2. Network egress from the executor is denied by default and allowed per destination.
3. Every tool call, command, network request and file change is an audit span written **before** it runs. If the audit write fails, the call does not run.
4. Tool output is untrusted text. It can never change settings, create a client, or reach the admin API.
5. Per-user token limits and the kill switch cover tool use.
6. The popup window still gets no filesystem or network permission.

## Alternatives considered

- **Stay conversation only for good.** Simplest, and keeps the audit story small. Loses the research and code features.
- **Tools inside `askbar-server`.** Rejected here: it puts untrusted execution in the container that holds provider keys and the audit store.

## Questions to answer before accepting

1. Are tools in scope for 1.0, or after?
2. Where does the executor run: same host, or a separate machine (the NAS)? If separate, how is it reached and authenticated?
3. Is "fail closed on audit failure" acceptable for tool calls?
4. Which tools first: web research (fetch and search) or code execution?

## Consequences

If accepted: [AGENTS.md](../../AGENTS.md) and [logging-and-audit.md](../style/logging-and-audit.md) drop their "proposed" tags for tools, and `scripts/check-isolation.sh` gains checks for each invariant.
