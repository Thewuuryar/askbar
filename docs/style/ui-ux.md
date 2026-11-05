# UI and UX

How the popup and the admin site behave. Items marked **(proposed)** are not decided.

## The popup (`ui/`, Tauri)

- **A bar, not an app.** One hotkey shows and hides it. Esc stops a streaming reply; otherwise it hides. Default hotkey `Ctrl+Shift+Space`.
- **Keyboard first.** Every action has a key (README, "Using it"). Ctrl is used on macOS too, because Cmd+H and Cmd+N belong to the system.
- **Thin client.** The window has no filesystem or network permission (`src-tauri/capabilities/default.json`). Only the Rust side talks to the server. Provider keys never reach the client.
- **No offline mode.** If the server cannot be reached the window says "server offline". It does not queue work silently or show stale answers as current.
- **Say it in words, keep what was typed.** A refusal (kill switch, token limit, no audit key, `audit_backlog`, queue full) is shown as a sentence and the text the person typed stays in the box. A cut-off reply keeps its text and offers **Try again**.
- **Branch on codes.** The app decides what to do from the `error` code, never from the wording. The text shown comes from the problem's `detail`.
- **Destructive actions take two clicks.** Delete asks "Delete" then "Delete?".
- **Pick a model from the approved list.** Clients choose from the models the admin approved, one of which is the default. The footer shows the model in use. There is no provider fallback in v1, so a failed provider is an error sentence, not a different answer ([ADR 0014](../adr/0014-model-gateway-and-providers.md)).
- **Notices are on by default and the admin can edit or turn them off** ([ADR 0012](../adr/0012-consent-notices-and-deletion.md)). The AI warning ("AI can make mistakes. Check important information.") sits above the chat box. The privacy notice shows at pairing and when the relevant settings change.
- **Live only where it started.** The client that started a chat streams the reply. Other clients show a processing message with a short description until the chat completes ([ADR 0008](../adr/0008-users-clients-and-client-access.md)).
- **No build step.** `ui/` is plain HTML, CSS and JavaScript. Adding a framework or bundler needs an ADR.
- **Markdown links and images.** Remote images are blocked. A link shows its destination before it opens ([ADR 0016](../adr/0016-risk-register.md); a ship criterion).
- **Markdown is escaped first.** `ui/markdown.js` HTML-escapes everything before it adds markup, so model output cannot inject HTML. That is the only reason `renderMarkdown` output may be assigned to `innerHTML`. Keep `tests/markdown.test.js` covering it.
- **Colours and sizes come from CSS variables** in `ui/style.css` (`--card`, `--fg`, `--accent`, ...). Do not hard-code a colour in a rule or in script. Themes are `auto`, `light`, `dark`.
- **Tests.** Behaviour changes carry a test in `tests/ui.test.js` (Playwright against `tests/mock-tauri.js`).

## The admin site (`askbar-admin/src/ui/`)

- Text goes in through `textContent`, never `innerHTML`. The CSP forbids inline script and style; do not loosen it.
- Anything that interrupts something says what it will interrupt before it asks (the Services tab does this).
- Show a secret once, at creation (API key, pairing string, token), and say so.
- A change made while the server is down is refused with a sentence, not half applied. The admin does not queue it ([ADR 0028](../adr/0028-service-boundaries-and-config-delivery.md)); it pushes the saved state when the server returns.
- Sealed content ([ADR 0011](../adr/0011-sealed-content-access.md)): the session shows its remaining time and a **Seal** button that ends it at once. Opening and timeout changes raise an alert; the screen says so before it asks for the passphrase.
- Tests: `tests/admin.test.js`, in a real browser against the real server and admin.

## Other clients **(proposed)**

- An iPhone app (native Swift) uses the same API, the same error codes and the same glossary names as the desktop app, and the same rules above: server offline in words, typed text kept, no keys on the phone.
- The admin site is for a desktop browser; it does not need a phone layout.
