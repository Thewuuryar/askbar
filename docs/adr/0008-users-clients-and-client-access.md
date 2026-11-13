# 0008. Users, clients, and client access

Status: Accepted 
Date: 2026-11-04
Plan decisions: 28, 29, 31

## Context

Users need simple client setup and private access away from home. 

## Decision

- **Users:** the admin creates users in the interface and generates a pairing key for each.
- **Clients:** a user registers one client with that key and requests more keys for additional clients.
- **Chats:** chats are processed on the server and sent to clients when complete.
- **Live updates:** live sync happens only on the client that started the chat, or it shows a processing message with a short description of what the chat is doing.
- **Network access:** clients reach the server over HTTPS on the one published port, from wherever that port is reachable. Askbar puts no address limit on the client API (it does on the admin, [0007](0007-first-run-and-admin-sign-in.md)): the client token and the version gate protect it. The default deployment keeps it on the local network or Tailscale because nothing forwards the port to the internet. Public access, by forwarding the port or hosting it publicly, is the deployer's choice.
- **Platforms:** macOS, Windows and Linux, each tested by a person before v1 ships ([0002](0002-v1-scope-limits-and-ship-criteria.md)). iOS is preferred, but can be deferred.

## Alternatives considered

- **Public client access by default:** rejected. It stays a deployer option.
- **Live sync across all clients:** rejected for v1.

## Consequences

The default deployment (no forwarded port) has a smaller attack surface, because of where it is installed and not because Caddy refuses other addresses. Clients other than the one that started a chat receive it when it's complete. A lost or stolen client must be revocable ([0016](0016-risk-register.md)).

## Amendments

- 2026-11-10: the platform and testing lines are one, linking 0002's ship checklist. No decision changed.
- 2026-11-13: the network-access line says what the code does (PR AW2, #108; **decision change**, proposed for the owner to keep or drop). It said clients use the local network or Tailscale "by default", which read as a limit Askbar enforces, but `services/admin/src/caddy.rs` puts a source allowlist only on the admin routes; the client routes have none. The line now says the client API is reachable from wherever the port is, is protected by the client token and the version gate, and is private by default only because the default deployment does not forward the port. A client allowlist like the admin's (private ranges unless set) is the alternative; it is not built, and would change `caddy.rs`, the admin's Network page and this ADR again.
