# 0015. Certificates

Status: Accepted 
Date: 2026-11-04
Plan decisions: 20, 34

## Context

Deployments may use a private address or a public host, and a domain isn't required except for a passkey.

## Decision

V1 keeps Caddy's Cloudflare DNS plugin and adds HTTP-01.

| Option | Works with | Needs | Private address |
| --- | --- | --- | --- |
| Cloudflare DNS | Cloudflare | API token limited to one zone | Yes |
| HTTP-01 | Any DNS provider | Public port 80 | No |

Add other DNS plugins or the acme-dns module only when someone needs them.

## Alternatives considered

- **More DNS plugins now:** rejected until someone needs them, because each adds a build dependency and testing.
- **acme-dns now:** rejected until someone needs it, because it needs a service to run or trust.

## Consequences

Private deployments that want a real certificate use Cloudflare DNS. Without a real certificate, the self-signed certificate from [0007](0007-first-run-and-admin-sign-in.md) stays in use.

## Amendments
