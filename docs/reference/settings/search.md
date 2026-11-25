---
id: settings/search
title: Search
summary: Whether quick search is on, its engines and limits, the models it uses, and what it has used.
keywords: [search, quick search, searxng, limits, usage, engines]
settings: [search]
---

# Search

Quick search answers a message from the web. This page holds its switch, its limits and what it has used.

## Quick search

The switch turns web search off for everyone at once; answers then say the search is off. Each person's own permission is on [MCP](mcp.md). The four limits (searches per question, characters per search, pages read per question and searches per person per hour) each show their value, default and range; Save keeps a new value and Reset goes back to the default. The two models the search jobs use are chosen under Model for each job on [Providers](providers.md).

## Search engine

Whether searxng answered the MCP front's last search (the admin cannot reach searxng itself), and the engines searxng asks with the egress host each one needs. A failed last search shows as down until a search works again; before any search it says so.

## MCP limits

The limits of the MCP front: page size, time limits, redirects, result counts and the lifetime of an approval. Each shows its value, default and range.

## Usage

Searches, fetches and refusals per day, and the turns, tokens and cost of the two model jobs (writing the query, writing the answer). It is read from the audit trail, shows no query, address or page text, and counts the newest 200 conversations at most.
