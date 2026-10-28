# Askbar

A Spotlight-style AI popup. Press a hotkey, a bar appears in the upper-middle of the screen, type a question, read the answer, press Esc.

- **Conversation only.** The model gets text in and returns text out. There are no tools, no shell, no file access, and the window is granted no filesystem or network permissions (`src-tauri/capabilities/default.json`). Only the Rust side talks to your provider.
- **Any provider.** Anthropic, or anything that speaks the OpenAI `/chat/completions` protocol (OpenAI, OpenRouter, Ollama, LM Studio, vLLM, Groq, ...).
- **History** you can toggle (Ctrl+H), reopen, and delete. Stored in a local SQLite file.
- **Copy in and out.** Paste anything into the bar. Copy a whole reply, a single code block, or the last reply (Ctrl+Shift+C).
- **Settings in one YAML file.**

This is milestones 1 and 2 of the plan: hotkey + window + streaming, and SQLite history. Projects, a settings UI, and self-improving memory are not built yet (see the end).

## Run it

Prerequisites: [Rust](https://rustup.rs), Node 18+, and your platform's WebView.

| Platform | Needs |
|---|---|
| Linux | `build-essential libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev` |
| macOS | Xcode command line tools |
| Windows | Visual Studio Build Tools (C++), WebView2 (preinstalled on Windows 11) |

```sh
npm install
npm run dev        # run it
npm run build      # installers land in target/release/bundle/
```

The first launch writes a commented config file and shows a welcome message with its path. Add your API key, then ask.

## Config

`~/.config/askbar/config.yaml` (Linux), `~/Library/Application Support/askbar/config.yaml` (macOS), `%APPDATA%\askbar\config.yaml` (Windows). Override with `ASKBAR_CONFIG=/path/to/file.yaml`. Tray icon → **Open config file**.

```yaml
hotkey: "Ctrl+Shift+Space"
active_provider: anthropic

providers:
  - name: anthropic
    type: anthropic            # anthropic | openai
    base_url: https://api.anthropic.com
    api_key_env: ANTHROPIC_API_KEY   # or put the key in api_key
    model: claude-sonnet-5-5
  - name: ollama
    type: openai
    base_url: http://localhost:11434/v1
    model: llama3.2
```

Keys in `api_key` are stored as plain text in that file; `api_key_env` reads an environment variable instead (an OS keychain option is a natural next step).

Provider, model, key and prompt edits apply on your next message. `hotkey` and `ui` changes apply from tray icon → **Reload config**. A YAML mistake is reported in the window with the file and line.

## Using it

| Key | Action |
|---|---|
| your hotkey | show / hide |
| Enter / Shift+Enter | send / new line |
| Esc | stop a reply that is streaming; otherwise hide |
| Ctrl+H | history panel |
| Ctrl+N | new conversation |
| Ctrl+Shift+C | copy the last reply |

In the history panel, click a conversation to reopen it. Delete needs two clicks ("Delete" then "Delete?"), and removes the conversation and its messages. Ctrl is used on macOS too, since Cmd+H and Cmd+N belong to the system.

Your data: `history.db` in the platform data directory (`~/.local/share/askbar/` on Linux). Override with `ASKBAR_DB`.

## Hotkeys by platform

`Super+A` is accepted (`hotkey: "Super+A"`) and works on X11, but Windows reserves it for Quick Settings and GNOME for the app grid, so the default is `Ctrl+Shift+Space`. If a hotkey can't be registered, the window opens with the reason.

**Wayland** does not allow ordinary apps to register global hotkeys. Bind a shortcut in your desktop's keyboard settings to the command `askbar`. Running it while Askbar is already running toggles the window instead of starting a second copy. This also works on any platform.

## Layout

```
core/        config.rs (YAML), db.rs (SQLite + migrations), provider.rs (SSE streaming)
             no GUI dependencies; unit + mock-server tests
src-tauri/   window, hotkey, tray, and the commands the UI can call
ui/          index.html, style.css, main.js, markdown.js (no build step)
tests/       markdown + UI tests, a mock LLM server, icon generator
```

## Tests

```sh
cargo test -p askbar-core        # config, history, SSE parsing, and both protocols against a mock server
node tests/markdown.test.js
node tests/ui.test.js            # needs: npm install && npx playwright install chromium
python3 tests/mock_llm.py 8765   # fake OpenAI-compatible server to try the app without a key
```

## Verified, and not

Verified on Linux (X11) by driving the real binary: hotkey show/hide, second-launch toggle, typing, streaming, Esc-to-stop (the server sees the disconnect and the partial reply is saved), deleting through real clicks with the database checked, clipboard copy and paste, first-run config, a bad hotkey, and hide-on-click-away.

Not verified: **Windows and macOS** (no machines here; `.github/workflows/build.yml` will build and test them), **Wayland**, and the live Anthropic and OpenAI APIs (both protocols are tested against a mock that follows their streaming formats). On macOS the dock icon is hidden by default; if Cmd+C or Cmd+V misbehave, set `ui.macos_hide_dock_icon: false`.

## Not built yet

- **Projects**: group conversations, move them between projects.
- **Settings UI**: today it is the YAML file only.
- **Self-improving memory**: after a conversation, a cheap model call proposes add/update/delete operations on a list of durable facts; the top matches are injected into the next system prompt; a periodic pass merges duplicates and drops stale entries; a view lets you edit or delete anything it learned. The schema has a migration slot ready.
- Conversation search; renaming.
