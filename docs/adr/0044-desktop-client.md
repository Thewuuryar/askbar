# 0044. The desktop client

Status: Accepted
Date: 2026-11-08
Kind: component
Source: plan step 1.14 ([plan-v1.md](../plan-v1.md)); owner rule that every component has an ADR of its own (2026-11-07)

## Context

The desktop app is how a person uses Askbar: a popup opened with a hotkey that asks a question and shows the streamed reply. It existed before the rule that each component has its own ADR. This record describes it as it is on `main` after plan step 1.11. It follows [0008](0008-users-clients-and-client-access.md), [0009](0009-spaces-and-storage.md), [0022](0022-api-path-conventions.md), [0023](0023-api-error-format.md), [0033](0033-repository-layout.md) and [0034](0034-self-signed-https-and-certificate-pinning.md), and links them instead of repeating them.

## Decision

- **What it is:** `clients/desktop`, a Tauri 2 app with its own Cargo workspace (`src-tauri/`), its web UI (`ui/`) and its tests (`tests/`) ([0033](0033-repository-layout.md)). It targets macOS, Windows and Linux ([0008](0008-users-clients-and-client-access.md)). It runs on the person's computer, outside the stack: it is not a container.
- **What it talks to:** only the client API, `/api/client/v1`, through Caddy over HTTPS. Every request carries `X-Askbar-Client-Version`; a server that answers 426 asks the person to update. It works in the user's personal space, looked up once per pairing with `GET /spaces` ([0009](0009-spaces-and-storage.md)). It branches on a problem's `error` code, never on its text ([0023](0023-api-error-format.md)).
- **Pairing and trust:** a pairing key gives the server address, a one-time code and, while the install uses its own certificate, that certificate's SHA-256. The app then trusts exactly that certificate ([0034](0034-self-signed-https-and-certificate-pinning.md)); without a pin it uses the public roots. Plain `http` is refused unless `allow_insecure` is set, which is for development only.
- **What it keeps:**
  - the client token, in the operating system's keychain only;
  - `client.yaml` in the user's config directory: the server address, the certificate pin, the hotkey and local display overrides. It holds no secret;
  - the outbox, in the app's data directory: the text of messages sent and not yet taken by the server, so a message is never lost. There is no offline chat.
- **Replies:** a reply runs on the server, so the app can close mid-reply and reattach with `?after=` from the last delta it saw. Stop asks the server to cancel.
- **Rendering:** the popup's content security policy allows only its own scripts. Only the output of `renderMarkdown` reaches `innerHTML`; everything else is set as text.

## Alternatives considered

Not recorded: the app predates this record.

## Consequences

The app's Rust tests run against a real server built from this repository and the mock (`clients/desktop/src-tauri`), and `tests/ui.test.js` drives the popup in a browser with a mocked Tauri layer. A person runs it on macOS, Windows and Linux before v1 ships ([0008](0008-users-clients-and-client-access.md)); only Linux runs in this repository's tests today.

**glib stays at 0.18 (owner decision, 2026-11-08).** The Linux build links glib 0.18 through Tauri's GTK 3 stack (`tauri`, `tao`, `wry`, `muda` and `tray-icon` need `gtk` 0.18, and `wry` pins `webkit2gtk` 2.0.2). No published Tauri release, up to 3.0.0-alpha.6, allows glib 0.20 or later, so a security alert asking for it is accepted with this reason until Tauri moves its Linux backend to newer bindings. The macOS and Windows builds and the server-side images contain no glib, and the app's own code does not call it. Patching glib 0.18 with a fork, or replacing the Linux backend, was not chosen. Check again on every Tauri update.

**Where the code and an accepted ADR disagree:**

- [0009](0009-spaces-and-storage.md): the app uses the personal space only and its outbox records no space; choosing a shared space is plan step 1.12.
- [0008](0008-users-clients-and-client-access.md): the macOS, Windows and Linux test by a person has not been done for this version.

## Amendments

None yet.
