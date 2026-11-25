---
id: settings/mcp
title: MCP
summary: Who may use Search and Fetch, the MCP front's state, its approval key, and where Search and Fetch may connect.
keywords: [MCP, search, fetch, permissions, approval key, egress]
settings: [mcp]
---

# MCP

Who may use Search and Fetch, the MCP front's state, its approval key, and where Search and Fetch may connect.

## Tool permissions

Turn Search or Fetch on for a person. Both are off until you do. A tool denied through the API stays off here.

## Front status

Whether the MCP front answers, the configuration version the admin pushed and the one the front reports, and how many calls ended in each outcome. A front with no configuration refuses every call.

## Approval key

The key that signs one-time approvals. Only its fingerprint (the first 16 hex digits of the SHA-256 of the public key) and version are shown. Rotating it needs a sign-in within the last 10 minutes, is recorded in the audit log, and takes effect at once: approvals signed with the old key stop working.

## Egress status

The destinations the admin allows `searxng` and `fetch`, and how many requests the egress proxy allowed and denied for each since it started.
