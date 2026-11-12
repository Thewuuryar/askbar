# 0044. The desktop client

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

The desktop app is how a person uses Askbar: a popup opened with a hotkey that asks a question and shows the streamed reply. It existed before the rule that each component has its own ADR. This record describes it as it is on `main` after plan step 1.11. It follows [0008](0008-users-clients-and-client-access.md), [0009](0009-spaces-and-storage.md), [0022](0022-api-path-conventions.md), [0023](0023-api-error-format.md), [0033](0033-repository-layout.md) and [0015](0015-certificates.md), and links them instead of repeating them.

## Decision

- **What it is:** `clients/desktop`, a Tauri 2 app with its own Cargo workspace (`src-tauri/`), its web UI (`ui/`) and its tests (`tests/`) ([0033](0033-repository-layout.md)). It targets macOS, Windows and Linux ([0008](0008-users-clients-and-client-access.md)). It runs on the person's computer, outside the stack: it is not a container.
- **What it talks to:** only the client API, `/api/client/v1`, through Caddy over HTTPS. Every request carries `X-Askbar-Client-Version`; a server that answers 426 asks the person to update. It works in the user's personal space, looked up once per pairing with `GET /spaces` ([0009](0009-spaces-and-storage.md)). It branches on a problem's `error` code, never on its text ([0023](0023-api-error-format.md)).
- **Pairing and trust:** a pairing key gives the server address, a one-time code and, while the install uses its own certificate, that certificate's SHA-256. The app then trusts exactly that certificate ([0015](0015-certificates.md)); without a pin it uses the public roots. Plain `http` is refused unless `allow_insecure` is set, which is for development only.
- **What it keeps:**
  - the client token, in the operating system's keychain only;
  - `client.yaml` in the user's config directory: the server address, the certificate pin, the hotkey and local display overrides. It holds no secret;
  - the outbox, in the app's data directory: the text of messages sent and not yet taken by the server, so a message is never lost. There is no offline chat.
- **Replies:** a reply runs on the server, so the app can close mid-reply and reattach with `?after=` from the last delta it saw. Stop asks the server to cancel.
- **From plan step 5.1 (notice and warning):** the snapshot's `policy` reaches the client; the popup shows the privacy notice after pairing and whenever its version (a hash of its text and the retention days) changes, and the AI warning above the input box, both as text. If 0012 requires it, it records the acknowledgment through an additive route ([0012](0012-consent-notices-and-deletion.md)).
- **From plan step 5.2 (links, images, Trusted Types):** a confirm bar shows the full URL before a link opens; `open_url` opens `http` and `https` only; remote image syntax renders as a link, so no remote resource is fetched. The popup's CSP requires Trusted Types, with one policy around the single `innerHTML` in `main.js` ([0016](0016-risk-register.md)).
- **Rendering:** the popup's content security policy allows only its own scripts. Only the output of `renderMarkdown` reaches `innerHTML`; everything else is set as text. Under each message a meta row shows its date and time and, on a reply, a chip whose tooltip names the model and provider that wrote it.

## Alternatives considered

Not recorded: the app predates this record.

## Consequences

The app's Rust tests run against a real server built from this repository and the mock (`clients/desktop/src-tauri`), and `tests/ui.test.js` drives the popup in a browser with a mocked Tauri layer. A person runs it on macOS, Windows and Linux before v1 ships ([0008](0008-users-clients-and-client-access.md)); only Linux runs in this repository's tests today.

**glib stays at 0.18 (owner decision, 2026-11-08).** The Linux build links glib 0.18 through Tauri's GTK 3 stack (`tauri`, `tao`, `wry`, `muda` and `tray-icon` need `gtk` 0.18, and `wry` pins `webkit2gtk` 2.0.2). No published Tauri release, up to 3.0.0-alpha.6, allows glib 0.20 or later, so a security alert asking for it is accepted with this reason until Tauri moves its Linux backend to newer bindings. The macOS and Windows builds and the server-side images contain no glib, and the app's own code does not call it. Patching glib 0.18 with a fork, or replacing the Linux backend, was not chosen. Check again on every Tauri update.

**Where the code and an accepted ADR disagree:**

- [0009](0009-spaces-and-storage.md): the app uses the personal space only and its outbox records no space; choosing a shared space is plan step 1.12.
- [0008](0008-users-clients-and-client-access.md): the macOS, Windows and Linux test by a person has not been done for this version.

## Amendments

- 2026-11-08: each message shows its date and time, and each reply the model and provider that wrote it (plan step 1.13).
- 2026-11-10: a failed question stays marked, live and after reopening, with Try again and Edit for every failure kind (plan step 1.13).
- 2026-11-10: the history panel copies a whole conversation as Markdown (plan step 1.13).
- 2026-11-10: the popup can show a conversation newest first, under the input bar (`newest_first`; plan step 1.13).
- 2026-11-11: design for plan steps 5.1 and 5.2 (#70, D15, D17, D18): the privacy notice and the AI warning, the link confirm bar, http(s)-only `open_url`, images as links, and Trusted Types. The code PRs add the 'built' lines.
- 2026-11-12: built the link confirm bar, images as links and Trusted Types (plan step 5.2, PR AE, #83). Built so: a click on a link opens a bar with the whole address (`#link-confirm`, in a box that scrolls, not cut) and opens nothing until Open; Cancel and Esc close it, and the first Esc does not also hide the window; only an `http` or `https` address with no space or control character and at most 2048 characters reaches the bar, and `open_url` checks that again in the backend (`is_openable`). `renderMarkdown` turns `![alt](https://…)` into a link labelled with the alt text, or the address, so no remote resource is ever fetched; any other scheme stays text. It also drops the NUL character from its input, and nests quotes and lists 20 levels deep at most, then flat, so a hostile reply cannot confuse it or overflow the stack while it streams. `tauri.conf.json`'s CSP adds `require-trusted-types-for 'script'; trusted-types askbar-markdown`; `main.js` has that one policy (its `createHTML` is `renderMarkdown`) and one HTML sink, `setReply`; a user message is set with `textContent`; if the browser refuses the policy a reply shows as plain text, and a webview with no Trusted Types falls back to `renderMarkdown`'s own escaping. Tested: `tests/payloads/markdown.json` (49 hostile inputs, with `lib.js`'s allowlist of tags, attributes and values) in Node and in the real popup in Chromium (no request, no script), and `tests/csp.test.js`, which serves the real files with the real CSP in Chromium. macOS (WKWebView) and Linux (WebKitGTK) are not Chromium, and Tauri could not be built where this was written (no GTK): they are on the person-run checklist ([0002](0002-v1-scope-limits-and-ship-criteria.md)).
