# Medal Clips (`medal-clips` + `medal-dl`)

Super fast Medal.tv clip downloader for montages. Saves the original MP4 with no watermark.

## Desktop app (`medal-clips.exe`)
`gui\target\release\medal-clips.exe`

- **Log in with Medal** (opens Medal's sign-in window, or use a session token as fallback) to browse your own clip library in a visual grid with thumbnails and hover previews.
- **Pick what you want**: click to select, `Shift`+click for ranges, `Ctrl+A` for all, double-click to grab one right away, filter by **Not downloaded** or **Game**, and sort by **Newest / Oldest / Longest / Shortest**.
- **Paste or drop anywhere**: paste clip links (`Ctrl+V` anywhere in the window), drag-and-drop links or `.txt` files onto the window, paste a public profile link (`medal.tv/u/…`) to browse someone's clips, or turn on **Grab copied links** in the Downloads drawer.
- **4-way chunked CDN streaming + 6 parallel clips**: uses Medal's fast JSON endpoint (`api/content/{id}`) and parallel HTTP byte-range chunks for larger clips, saving raw watermark-free `.mp4` files to `Videos\Medal Clips`.

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
```
cd gui
cargo build --release   # -> gui\target\release\medal-clips.exe

cd ..
cargo build --release   # -> target\release\medal-dl.exe
```
