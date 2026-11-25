---
id: settings/mcp
title: MCP
summary: Who may use Search and Fetch, the MCP front's state, its approval key, and where Search and Fetch may connect.
keywords: [MCP, search, fetch, permissions, approval key, egress]
settings: [mcp]
---

# MCP

Who may use Search and Fetch, whether the MCP front answers, the key that signs its one-time approvals, and where Search and Fetch may connect. The front is the service that runs the two tools; the settings for their limits are on [Search](search.md#mcp-limits).

## Tool permissions

One row for each person, with a switch for Search and one for Fetch. Both are off for everyone until you turn one on. Turning a switch on adds the tool to that person's allowed list, and turning it off removes it again. A tool the API has denied for a person stays denied: the switch shows Denied and cannot be changed here, and the entry is left as it is. Each change is an audited admin action, and the front gets the new permissions with the next configuration push. Search and Fetch are also set through the API, as described in [People & clients](people.md#search-and-fetch-permissions).

## Front status

Whether the front answers, the configuration version the admin pushed and the version the front reports (they match once the front has taken the push), and the calls it has answered, counted by outcome. A front with no configuration refuses every call, and a banner at the top of the page says so: when the front answers, the admin pushes the configuration within seconds; when it does not, the banner gives the reason it last reported.

## Approval key

The key that signs the one-time approvals the front checks. The page shows only its fingerprint (the first 16 hex digits of the SHA-256 of the public key) and its version. No key is shown or recorded.

**Rotate the key** makes a new key under the next version. It needs a sign-in no older than 10 minutes: if your sign-in is older, the page asks you to sign out, sign in again and rotate within that time. The rotation is recorded in the audit log before it takes effect, and the admin is refused the rotation if that record cannot be made. The new public key goes to the front and the new private key goes to `model-gateway` straight away. From then on, an approval signed with the old key is refused, because the front checks each approval against the new public key.

## Egress status

The destinations the admin allows `searxng` and `fetch` through the [egress proxy](egress.md), and how many requests the proxy allowed and denied for each since it started. If the proxy does not answer, a note says so and the counts are from its last answer. Denied requests and the reasons are on [Egress](egress.md#blocked-outbound-requests).
