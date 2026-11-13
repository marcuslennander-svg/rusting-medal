# Rusting Medal — Roadmap

Living plan. Check items off in the same commit that completes them, and add new findings.
See [AGENTS.md](AGENTS.md) for conventions.

Legend: `[x]` done · `[ ]` todo · **P1** high, **P2** medium, **P3** nice-to-have

## v0.2 — current

### Fixes
- [x] **P1** Social logins (Discord, YouTube/Google, Steam, …) did nothing — popups were blocked
      by WebView2. Handled via `on_new_window` in `build_auth_window`.
- [x] **P1** Rename app to **Rusting Medal** (window title, brand, exe `rusting-medal.exe`, docs).

### Performance
- [x] **P1** Batch disk writes in the downloader (1 MiB buffer per chunk stream) instead of a
      `spawn_blocking` per network packet.
- [x] **P2** Debounce search input so typing doesn't re-render the whole grid per key.
- [x] **P2** Cache the "already saved" directory scan briefly instead of re-reading the folder for
      every queued clip.

### Quality of life
- [x] Retry failed downloads (click a failed item, or **Retry failed** in the drawer).
- [x] Overall progress in the window title (e.g. `Rusting Medal — 3 downloading · 42%`).
- [x] Remember sort order and All/Not-downloaded view between launches.
- [x] `Ctrl+F` or `/` focuses search; `Esc` clears search/selection.
- [x] Toast when a whole batch finishes ("Downloaded 12 clips").
- [x] Right-click a downloaded card to show it in the folder.

## v0.3 — next up
- [ ] **P1** In-app clip player: a play button on card hover (or `Space` on the hovered card)
      opens a lightbox player with the full-quality stream (`clip.url`, fall back to
      `previewUrl`): play/pause, scrub, volume, `←/→` to step to the previous/next clip in the
      current filtered list, `D` to download, `S` to toggle selection, `Esc` to close. Plays the
      local file instead if the clip is already saved (needs Tauri asset protocol scope for the
      output folder). Click-to-select on cards must keep working as today.
- [ ] **P1** Cancel a single download / cancel all (needs an abort handle per job in `AppState`).
- [ ] **P1** Remember window size and position (`tauri-plugin-window-state`).
- [ ] **P2** Single-instance: launching the exe again focuses the running window
      (`tauri-plugin-single-instance`), and forwards medal links passed as args.
- [ ] **P2** Windows toast notification when a batch completes while the app is unfocused
      (`tauri-plugin-notification`).
- [ ] **P2** Settings panel: parallel clips (1–12), chunks per clip, filename template preview.
- [ ] **P2** Show download speed (MB/s) and ETA per item and overall.
- [ ] **P3** Date-range filter ("last 7 days", "this month").
- [ ] **P3** "Select not downloaded" button.

## v0.4 — later
- [ ] **P2** Installer / auto-update (Tauri bundler + updater plugin, GitHub Releases).
- [ ] **P2** GitHub Actions: build both exes on tag and attach to a release.
- [ ] **P2** Share a common `medal-core` crate between CLI and GUI (turn repo into a Cargo
      workspace) to remove duplicated parsing/config code.
- [ ] **P3** Optional subfolders per game (`Medal Clips\Valorant\…`).
- [ ] **P3** Custom app icon matching the new name.
- [ ] **P3** Light theme.

## Known issues / notes
- Google (and therefore YouTube) login may show "This browser or app may not be secure" inside
  embedded WebView2. Workaround: log in with Discord/email, or use the session-token dialog.
- Medal's private API (`/api/content`, `/api/users`) is undocumented and may change. Parsing is
  defensive (`ClipInfo::from_api`) — extend it rather than replacing it.
- The Tauri `identifier` is still `dev.medalclips.app` on purpose: changing it would move the
  WebView2 profile and log every user out of the embedded login window.
