# Architecture decision records

One short file per decision that a later contributor, human or agent, could otherwise reopen. [architecture.md](../architecture.md) says what the system is; an ADR says why a choice was made and what it costs.

## Rules

- File name `NNNN-short-title.md`, numbered in order, never reused.
- Status is `Proposed`, `Accepted`, `Superseded by NNNN` or `Rejected`. Do not rewrite an accepted ADR; write a new one that supersedes it.
- Write one when a change crosses a boundary (crate, container, network, volume, API audience), adds a dependency or a container, or reverses something recorded here. Keep it under a page.
- Do not repeat what [architecture.md](../architecture.md) or [SECURITY.md](../../SECURITY.md) already say; link to it.
- "Not recorded" means the reasoning was not written down when the decision was made. Do not invent it.
- Copy [0000-template.md](0000-template.md).

## Index

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](0002-three-audiences-one-host.md) | Three audiences, one vocabulary, one host name | Accepted |
| [0003](0003-admin-and-server-isolation.md) | The admin and the server are isolated | Accepted |
| [0004](0004-sealed-audit-content.md) | Audit content is sealed | Accepted |
| [0005](0005-docker-socket-only-in-controller.md) | The Docker socket lives only in the controller | Accepted |
| [0006](0006-problem-details-errors.md) | One error shape with stable codes | Accepted |
| [0007](0007-conversation-only-and-tool-gating.md) | Conversation only, and what gates tools | Proposed |
| [0008](0008-admin-sign-in-and-exposure.md) | Admin sign-in and exposure | Proposed |
