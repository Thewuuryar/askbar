# 0020. Remote executor and CA triggers

Status: Accepted
Date: 2026-11-04
Plan decisions: 24

## Context

The executor will run on the home network or a VPS. It needs a private connection without a public endpoint or, by default, a CA.

## Decision

V2 keeps per-link tokens unless a CA trigger is met. The executor runs what it's handed and returns output, and never calls a model or an MCP server.

**Pairing**

- The admin creates a one-time code, the executor makes its own key pair, and it registers the public key. The private key never leaves that machine.
- Executor pairing has its own credential scope and API audience, separate from client permissions.
- The admin can revoke one executor at once.
- The tool rules in [0019](0019-post-v1-order-and-tools.md) apply, including no route to the admin or server API.

**Connection direction:** the executor may open the connection, but only to receive work and return output.

**Private tunnel**

1. Run Tailscale on the executor host, with no public port for the executor.
2. Add an optional `executor-link` Compose service. It runs Tailscale and forwards one port to the server's executor endpoint. Tailscale stays out of the `server` container.
3. Tag the nodes, for example `tag:askbar-link` and `tag:askbar-executor`.
4. Allow only `tag:askbar-executor` to reach `tag:askbar-link` on that port. Add no rule from the executor to other machines, or from Askbar to the executor.
5. Use a tagged OAuth client secret from a file, and keep the state directory on its own volume. Don't put an auth key in an environment variable.
6. Test that the executor has no public port, can reach only that one port on Askbar, can't reach other machines, and is refused after revocation.

**CA triggers:** move to a CA if a requirement demands short-lived credentials or mTLS, or if there are more remote components than the owner can comfortably re-pair by hand. If a CA is adopted, keep authentication behind a shared interface, migrate one link at a time, use longer certificates, and keep the root key off the admin.

## Alternatives considered

- **A route through Caddy:** rejected, because it adds an internet-facing endpoint.
- **The server calling the executor:** rejected. The executor may open the connection.
- **A private CA for the executor:** deferred until a trigger is met.

## Consequences

Tailscale needs one manual setup for its tags, rules, and OAuth client. Pairing still proves which executor is connected, because the tunnel provides only the private route. This ADR's details are finalized when executor work starts.
