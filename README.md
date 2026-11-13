# Rusting Medal

Super fast Medal.tv clip downloader for montages, written in Rust. Saves the original MP4 with no watermark.

- **Desktop app:** `rusting-medal.exe` (Tauri 2)
- **CLI:** `medal-dl.exe`

> Working on the code (human or AI)? Read [AGENTS.md](AGENTS.md) and [ROADMAP.md](ROADMAP.md) first.

## Desktop app (`rusting-medal.exe`)
`gui\target\release\rusting-medal.exe`

- **Log in with Medal**: opens Medal's sign-in window. Email, Discord, Steam and the other social logins all work there. If Google/YouTube refuses the embedded window, use **Log in with a session token** instead.
- **Browse your library** in a grid with thumbnails and hover previews. Filter by **Not downloaded** or **Game**, and sort by **Newest / Oldest / Longest / Shortest**. Sort and filter choices are remembered between launches.
- **Pick what you want**: click to select, `Shift`+click for ranges, double-click to download one right away, then press **Download**.
- **Paste or drop anywhere**: clip links, `.txt` files with links, or a public profile link (`medal.tv/u/…`) to browse someone else's clips. You can also turn on **Grab copied links** in the Downloads drawer.
- **Downloads drawer**: click a finished item to show it in Explorer, click a failed one to retry it, or use **Retry failed** to retry them all. The window title shows overall progress.
- **Fast**: 6 clips download in parallel, and bigger clips are split into 4 byte-range chunks. Files are saved as raw, watermark-free `.mp4` in `Videos\Medal Clips`.

### Shortcuts
| Key | Action |
| --- | --- |
| `Ctrl+A` | Select / deselect all visible clips |
| `Shift`+click | Select a range |
| Double-click | Download that clip now |
| Right-click | Show a downloaded clip in its folder |
| `Enter` | Download the selection |
| `Ctrl+F` or `/` | Search |
| `Esc` | Clear the search box, or clear the selection |
| `Ctrl+V` | Paste links anywhere in the window |

## CLI (`medal-dl.exe`)
`target\release\medal-dl.exe`

Double-click `medal-dl.exe` for interactive/clipboard mode, or run:
```
medal-dl <link> <link> ...        download these clips
medal-dl -f links.txt             grab every Medal link in a text file
medal-dl -o D:\Montage <links>    choose the output folder
medal-dl set-dir D:\Montage       always save here
medal-dl -j 10 ...                download 10 clips at the same time (default 6)

medal-dl login                    log in once
medal-dl mine                     download ALL your clips (skips ones already saved)
medal-dl mine --limit 20          only your newest 20
```

## Build from source
Requires Rust stable (MSVC) and the Visual Studio C++ Build Tools. Node isn't needed.
```
cd gui
cargo build --release   # -> gui\target\release\rusting-medal.exe

cd ..
cargo build --release   # -> target\release\medal-dl.exe
```
