---
id: settings/search
title: Search
summary: Whether quick search is on, its engines and limits, the models it uses, and what it has used.
keywords: [search, quick search, searxng, limits, usage, engines]
settings: [search]
---

# Search

Quick search answers a message from the web. This page holds its switch, its limits and what it has used. A person can search only when the admin has given them the Search permission on [MCP](mcp.md#tool-permissions) too.

## Quick search

The switch turns web search off for everyone at once. It is on by default, and it is separate from the kill switch, which stops every reply. While it is off, a reply that would have searched says the search is turned off by the administrator, and nothing is sent to the search engines. Turning it on again applies at the next configuration push, with no restart.

The four quick-search limits are below. Each one shows its value. Save keeps a new value, and Reset goes back to the default. A value outside its range is refused, and the reason is shown as text. Each change is an audited admin action.

| Limit | Name in the API | Default | Minimum | Maximum | Unit |
|---|---|---|---|---|---|
| Searches per question | `max_queries` | 3 | 1 | 5 | count |
| Characters per search | `max_query_chars` | 200 | 20 | 200 | characters |
| Pages read per question | `max_pages` | 3 | 1 | 5 | count |
| Searches per person per hour | `searches_per_user_hour` | 10 | 1 | 100 | count |

The line above the table names the two models the search jobs use: one writes the query and one writes the answer. Pick them under Model for each job on [Providers](providers.md#model-for-each-job).

## Research

The Research switch beside the quick search switch turns research reports off for everyone at once. It is on by default and separate from the kill switch and from quick search. While it is off, the gateway refuses research calls and the server answers without research. Each change is an audited admin action.

The line above the table names the three models research uses: the researchers, the one that writes the report and the judge. Pick them under Model for each job on [Providers](providers.md#model-for-each-job). The judge should be on a provider other than the researchers'; this is a hint and nothing is enforced.

| Limit | Name in the API | Default | Minimum | Maximum | Unit |
|---|---|---|---|---|---|
| Tool calls per task | `tool_calls` | 8 | 1 | 32 | count |
| Tokens per task | `token_cap` | 300000 | 20000 | 2000000 | tokens |
| Researchers per task | `researchers_max` | 4 | 2 | 4 | count |
| Wait for the rest after two finish | `wait_after_two_secs` | 8 | 1 | 60 | seconds |
| Research time limit | `stage_secs` | 90 | 10 | 600 | seconds |
| Tokens per researcher summary | `summary_tokens` | 800 | 100 | 4000 | tokens |

Save, Reset and the range rule are as for the quick-search limits. A change reaches the gateway and the server on the next push. A task that reaches 75% (warning) or 90% (critical) of its token cap raises a `task_cap_threshold` alert, with no task or person named.

## Search engine

Whether the MCP front's last search worked, and the engines searxng asks, each with the egress host it needs. The front records its own last real search; the admin does not probe searxng. So an idle searxng that is down shows as answering until a search runs, and a search that fails shows as down at the next poll, about 12 seconds later. Before any search since the front started it says No search yet. If the front does not answer, the state is Not known.

| Engine | Egress host |
|---|---|
| DuckDuckGo | `html.duckduckgo.com` |
| Brave | `search.brave.com` |
| Yahoo | `search.yahoo.com` |
| mwmbl | `api.mwmbl.org` |

These are fixed with the install: a test checks that each engine has its host and that no host lacks an engine. Nothing is typed here. The hosts are allowed on [Egress](egress.md#allowlist).

## MCP limits

The limits of the MCP front, which runs Search and Fetch. Each shows its value, with Save and Reset, and the same range rule as the quick-search limits.

| Limit | Name in the API | Default | Minimum | Maximum | Unit |
|---|---|---|---|---|---|
| Largest page | `fetch_max_bytes` | 1048576 | 65536 | 5242880 | bytes |
| Fetch time limit | `fetch_timeout_secs` | 20 | 1 | 60 | seconds |
| Redirects followed | `fetch_redirects` | 3 | 0 | 5 | count |
| Tool call time limit | `tool_call_timeout_secs` | 30 | 5 | 120 | seconds |
| Approval lifetime | `one_time_id_window_secs` | 120 | 10 | 600 | seconds |
| Results per search | `search_results` | 8 | 1 | 20 | count |
| Result links remembered per task | `remembered_urls` | 50 | 10 | 200 | count |
| Largest tool result | `tool_result_max_bytes` | 1048576 | 65536 | 8388608 | bytes |

The approval lifetime is how long a one-time ID from a search can be used. Each change reaches the front on its next push.

## Usage

Searches, fetches and refusals for each of the last 7 days, and for each model job (writing the query, writing the answer, and the three research jobs) the turns, tokens in and out, and cost per day. Cost uses the prices set on [Providers](providers.md). The figures come from the audit trail, not the server's database. They are read from the newest 200 conversations at most, and the note above the tables says when older days may be incomplete. Only counts, tokens and cost are shown: no query, address or page text.
