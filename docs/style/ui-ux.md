# UI and UX

How the popup and the admin site behave. Items marked **(proposed)** are not decided.

## The popup (`ui/`, Tauri)

- **A bar, not an app.** One hotkey shows and hides it. Esc stops a streaming reply; otherwise it hides. Default hotkey `Ctrl+Shift+Space`.
- **Keyboard first.** Every action has a key (README, "Using it"). Ctrl is used on macOS too, because Cmd+H and Cmd+N belong to the system.
- **Thin client.** The window has no filesystem or network permission (`src-tauri/capabilities/default.json`). Only the Rust side talks to the server. Provider keys never reach the client.
- **No offline mode.** If the server cannot be reached the window says "server offline". It does not queue work silently or show stale answers as current.
- **Say it in words, keep what was typed.** A refusal (kill switch, token limit, no audit key, queue full) is shown as a sentence and the text the person typed stays in the box. A cut-off reply keeps its text and offers **Try again**.
- **Branch on codes.** The app decides what to do from the `error` code, never from the wording. The text shown comes from the problem's `detail`.
- **Destructive actions take two clicks.** Delete asks "Delete" then "Delete?".
- **Say who answered.** If a provider other than the chosen one answered, the reply says "Answered by ...".
- **No build step.** `ui/` is plain HTML, CSS and JavaScript. Adding a framework or bundler needs an ADR.
- **Markdown is escaped first.** `ui/markdown.js` HTML-escapes everything before it adds markup, so model output cannot inject HTML. That is the only reason `renderMarkdown` output may be assigned to `innerHTML`. Keep `tests/markdown.test.js` covering it.
- **Colours and sizes come from CSS variables** in `ui/style.css` (`--card`, `--fg`, `--accent`, ...). Do not hard-code a colour in a rule or in script. Themes are `auto`, `light`, `dark`.
- **Tests.** Behaviour changes carry a test in `tests/ui.test.js` (Playwright against `tests/mock-tauri.js`).

## The admin site (`askbar-admin/src/ui/`)

- Text goes in through `textContent`, never `innerHTML`. The CSP forbids inline script and style; do not loosen it.
- Anything that interrupts something says what it will interrupt before it asks (the Services tab does this).
- Show a secret once, at creation (API key, pairing string, token), and say so.
- A change made while the server is down is refused with a sentence, not half applied.
- Tests: `tests/admin.test.js`, in a real browser against the real server and admin.

## Other clients **(proposed)**

- An iPhone app (native Swift) uses the same API, the same error codes and the same four words as the desktop app, and the same rules above: server offline in words, typed text kept, no keys on the phone.
- The admin site is for a desktop browser; it does not need a phone layout.
