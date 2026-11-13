# Guidelines for contributors & AI agents

Read this file and [ROADMAP.md](ROADMAP.md) before changing anything. Keep both up to date
as part of the same commit as your change.

## What this project is

**Rusting Medal** — a fast, native Windows app for picking and downloading your Medal.tv
clips (original, watermark-free MP4s) for montages. Two binaries live in this repo:

| Binary | Path | What it is |
| --- | --- | --- |
| `rusting-medal.exe` | `gui/` | Tauri 2 desktop app (Rust backend + vanilla HTML/CSS/JS UI) |
| `medal-dl.exe` | `/` (root crate, `src/main.rs`) | Standalone CLI downloader |

The two crates are **independent** (separate `Cargo.toml`/`Cargo.lock`, no workspace).
Some logic (link parsing, sanitizing, config) is duplicated on purpose so each binary stays
self-contained. If you fix a bug in one copy, fix it in the other too.

## Layout

```
gui/src/main.rs      Tauri commands, login window, download queue + chunked downloader
gui/src/medal.rs     Medal.tv API helpers, parsing, config (pure-ish, unit tested)
gui/ui/index.html    Static markup, SVG icon sprite, dialogs
gui/ui/app.js        All UI state (`S`) and rendering; talks to Rust via invoke()/listen()
gui/ui/style.css     Design tokens + components
gui/tauri.conf.json  Window + bundle config
src/main.rs          CLI (clap)
```

## Building

Requirements: Rust stable (MSVC toolchain), Visual Studio C++ Build Tools, WebView2 (ships with Windows 10/11).
No Node/npm is needed — the UI is plain static files served by Tauri.

```powershell
cd gui;  cargo build --release   # -> gui\target\release\rusting-medal.exe
cd ..;   cargo build --release   # -> target\release\medal-dl.exe
cargo test                        # run in both crates before committing
```

`cargo build --release` in `gui/` produces a working standalone exe because the
`custom-protocol` feature is enabled in `gui/Cargo.toml`. Do not remove it.

## Conventions (keep these consistent)

### General
- **Naming:** the product is "Rusting Medal" in every user-facing string (window titles,
  headings, errors, README). The CLI keeps its command name `medal-dl`.
- **Do not break user data:** the config lives in `%APPDATA%\medal-dl\config.json` and is shared
  by both binaries; default output folder is `Videos\Medal Clips`. Only add fields with
  `#[serde(default)]`; never rename/remove existing ones.
- Saved files are named `"{sanitized title} [{clipId}].mp4"`. The `[id]` suffix is how
  "already downloaded" detection works — never change this format.
- Downloads write to `*.mp4.part` and are renamed on success. Keep that atomic pattern.
- No new heavy dependencies without a reason written in the commit message. Prefer std / what's
  already in `Cargo.toml`.

### Rust
- `cargo fmt` style, `cargo clippy` clean where practical.
- Tauri commands return `Res<T>` (`Result<T, String>`); map errors with `err`.
- Errors shown to users must be plain English and actionable ("Log in again", not stack traces).
- Never hold a `std::sync::Mutex` guard across an `.await`.
- Long work goes in `tauri::async_runtime::spawn`; blocking file IO in `spawn_blocking`
  (batch it — don't spawn per tiny chunk).
- Backend → UI communication uses events with kebab-case names (`dl-queued`, `dl-progress`,
  `dl-done`, `dl-error`, `auth-changed`, `auth-error`, `clipboard-grabbed`). Payload structs
  use `#[serde(rename_all = "camelCase")]`.
- Add unit tests for any parsing/pure logic (see tests in `medal.rs`).

### UI (gui/ui)
- Vanilla JS, no frameworks, no build step, no CDN imports (the app must work offline).
- All state lives in the `S` object in `app.js`. Render functions are idempotent;
  prefer `patchCard` / `patchDl` for hot paths (progress) over full re-renders.
- **Always** pass dynamic text through `esc()` before putting it in `innerHTML`.
- Icons come from the SVG sprite in `index.html` (`icon("name")`); 24px grid, 1.75 stroke.
  Add new icons there, not inline.
- Use the CSS variables in `style.css` (`--accent`, `--bg-*`, `--text-*`, radii, spacing).
  Don't hard-code new colors.
- Copy tone: short, friendly, sentence case, no exclamation marks.
- Persist UI preferences in `localStorage` under keys prefixed `rm.`.
- Every new action should have a keyboard shortcut if it is used often; document it in the
  README shortcuts table.

### Login window
- The login window loads medal.tv and grabs the `medal-auth` cookie via `LOGIN_JS`, then
  hands it to Rust by navigating to `https://medal-dl.invalid/auth?...` (intercepted).
- Social logins (Discord, Google, Steam, …) open popups; these are handled by
  `on_new_window` in `build_auth_window`. Without that handler WebView2 silently drops popups.
- Google may refuse embedded browsers. The token-paste dialog is the supported fallback.

## Workflow for agents
1. Pick the top unchecked item from ROADMAP.md (or what the user asked for).
2. Make the change; keep diffs focused.
3. `cargo build --release` + `cargo test` in the crate(s) you touched.
4. Refresh the prebuilt binaries in `dist/` (`dist/rusting-medal.exe`, `dist/medal-dl.exe`)
   when shipping/pushing changes.
5. Update ROADMAP.md (check items off, add follow-ups you discovered) and README if user-facing.
6. Commit with a clear message: `area: what changed` (e.g. `gui: retry failed downloads`).
