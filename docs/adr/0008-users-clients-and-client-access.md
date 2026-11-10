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
- **Network access:** clients use the local network or Tailscale by default. Public access can be a deployer option.
- **Platforms:** macOS, Windows, and Linux. iOS is preferred, but can be deferred.
- **Testing:** a person tests the client on macOS, Windows, and Linux before v1 ships.

## Alternatives considered

- **Public client access by default:** rejected. It stays a deployer option.
- **Live sync across all clients:** rejected for v1.

## Consequences

The default deployment has a smaller attack surface. Clients other than the one that started a chat receive it when it's complete. A lost or stolen client must be revocable ([0016](0016-risk-register.md)).

## Amendments
