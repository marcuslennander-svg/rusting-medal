#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod medal;

use anyhow::anyhow;
use futures_util::StreamExt;
use medal::{ClipInfo, Config, ProfileLookup};
use reqwest::StatusCode;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const PARALLEL_CLIPS: usize = 6;
const PARALLEL_RESOLVE: usize = 16;
const FIRST_CHUNK_BYTES: u64 = 2 * 1024 * 1024; // 2 MiB initial probe+chunk
const MAX_CHUNKS_PER_CLIP: u64 = 4;

/// Runs inside the Medal login window. Once the user has signed in, Medal stores
/// the session in the `medal-auth` cookie; we hand it back to the app by
/// navigating to a fake host that the app intercepts (and cancels).
const LOGIN_JS: &str = r#"
(function () {
  if (window.top !== window) return;
  if (!/(^|\.)medal\.tv$/i.test(location.hostname)) return;
  var sent = false;
  function grab() {
    if (sent) return;
    var c = document.cookie.split('; ').find(function (x) { return x.indexOf('medal-auth=') === 0; });
    if (!c) return;
    var raw = c.slice(11), o = null;
    try { raw = decodeURIComponent(raw); } catch (e) {}
    try {
      var b64 = raw.replace(/-/g, '+').replace(/_/g, '/');
      o = JSON.parse(atob(b64));
    } catch (e) { try { o = JSON.parse(raw); } catch (e2) {} }
    if (o && o.auth && !o.key) { o.key = o.auth.key; o.userId = o.userId || o.auth.userId; }
    if (o && o.userId && o.key && o.registeredUser) {
      sent = true;
      location.href = 'https://medal-dl.invalid/auth?u=' + encodeURIComponent(o.userId) + '&k=' + encodeURIComponent(o.key);
    }
  }
  setInterval(grab, 500);
})();
"#;

struct AppState {
    cfg: Mutex<Config>,
    client: reqwest::Client,
    dl_sem: Arc<Semaphore>,
    resolve_sem: Arc<Semaphore>,
    active: Mutex<HashSet<String>>,
    clipboard_watch: Arc<AtomicBool>,
}

type Res<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    logged_in: bool,
    user_id: Option<String>,
    name: Option<String>,
    avatar: Option<String>,
    out_dir: String,
    clipboard_watch: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Queued {
    id: String,
    title: String,
    thumb: Option<String>,
}
#[derive(Serialize, Clone)]
struct Progress {
    id: String,
    done: u64,
    total: u64,
}
#[derive(Serialize, Clone)]
struct Done {
    id: String,
    path: String,
    bytes: u64,
    skipped: bool,
}
#[derive(Serialize, Clone)]
struct Failed {
    id: String,
    error: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkResult {
    queued: usize,
    profile: Option<String>,
}

// ------------------------------------------------------------------ commands

#[tauri::command]
async fn app_info(state: State<'_, AppState>) -> Res<AppInfo> {
    let cfg = state.cfg.lock().unwrap().clone();
    let user = match &cfg.auth {
        Some(t) => medal::user_info(&state.client, t).await,
        None => Default::default(),
    };
    Ok(AppInfo {
        logged_in: cfg.auth.is_some(),
        user_id: cfg.user_id(),
        name: user.name,
        avatar: user.avatar,
        out_dir: cfg.out_dir().display().to_string(),
        clipboard_watch: cfg.clipboard_watch,
    })
}

#[tauri::command]
async fn list_clips(
    state: State<'_, AppState>,
    user_id: Option<String>,
    offset: usize,
    limit: usize,
) -> Res<Vec<ClipInfo>> {
    let (token, my_uid, dir) = {
        let cfg = state.cfg.lock().unwrap();
        (cfg.auth.clone(), cfg.user_id(), cfg.out_dir())
    };
    let token = token.ok_or("NOT_LOGGED_IN")?;
    let target = user_id.or(my_uid).ok_or("NOT_LOGGED_IN")?;
    let mut clips = medal::list_clips(&state.client, &token, &target, offset, limit.clamp(1, 100))
        .await
        .map_err(err)?;
    let saved = medal::saved_ids(&dir);
    for c in &mut clips {
        c.saved = saved.contains_key(&c.id);
    }
    Ok(clips)
}

#[tauri::command]
async fn lookup_profile(state: State<'_, AppState>, username: String) -> Res<ProfileLookup> {
    let (logged_in, dir) = {
        let cfg = state.cfg.lock().unwrap();
        (cfg.auth.is_some(), cfg.out_dir())
    };
    let mut prof = medal::lookup_profile(&state.client, &username, !logged_in)
        .await
        .map_err(err)?;
    let saved = medal::saved_ids(&dir);
    for c in &mut prof.initial_clips {
        c.saved = saved.contains_key(&c.id);
    }
    Ok(prof)
}

#[tauri::command]
async fn open_login(app: AppHandle) -> Res<()> {
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.set_focus();
        return Ok(());
    }
    let handle = app.clone();
    WebviewWindowBuilder::new(
        &app,
        "login",
        WebviewUrl::External("https://medal.tv/login".parse().unwrap()),
    )
    .title("Log in to Medal")
    .inner_size(520.0, 780.0)
    .initialization_script(LOGIN_JS)
    .on_navigation(move |url| {
        if url.host_str() == Some("medal-dl.invalid") {
            let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
            if let (Some(u), Some(k)) = (q.get("u"), q.get("k")) {
                let token = format!("{u},{k}");
                let app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    match finish_login(&app, token).await {
                        Ok(()) => {
                            if let Some(w) = app.get_webview_window("login") {
                                let _ = w.close();
                            }
                        }
                        Err(e) => {
                            let _ = app.emit("auth-error", e);
                        }
                    }
                });
            }
            return false;
        }
        true
    })
    .build()
    .map_err(err)?;
    Ok(())
}

async fn finish_login(app: &AppHandle, token: String) -> Res<()> {
    let state = app.state::<AppState>();
    let uid = token.split(',').next().unwrap_or_default().to_string();
    if let Err(e) = medal::list_clips(&state.client, &token, &uid, 0, 1).await {
        if e.to_string() == "SESSION_EXPIRED" {
            return Err("Medal didn't accept that session. Try logging in again.".into());
        }
        return Err(format!("Couldn't verify session with Medal: {e}"));
    }
    {
        let mut cfg = state.cfg.lock().unwrap();
        cfg.auth = Some(token);
        cfg.save().map_err(err)?;
    }
    let _ = app.emit("auth-changed", ());
    Ok(())
}

#[tauri::command]
async fn login_with_token(app: AppHandle, token: String) -> Res<()> {
    let token = medal::parse_token(&token).map_err(err)?;
    finish_login(&app, token).await
}

#[tauri::command]
fn logout(app: AppHandle, state: State<'_, AppState>) -> Res<()> {
    let mut cfg = state.cfg.lock().unwrap();
    cfg.auth = None;
    cfg.save().map_err(err)?;
    let _ = app.emit("auth-changed", ());
    Ok(())
}

#[tauri::command]
async fn pick_dir(state: State<'_, AppState>) -> Res<Option<String>> {
    let start = state.cfg.lock().unwrap().out_dir();
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Where should clips be saved?")
        .set_directory(&start)
        .pick_folder()
        .await;
    let Some(dir) = picked else { return Ok(None) };
    let path = dir.path().to_path_buf();
    let mut cfg = state.cfg.lock().unwrap();
    cfg.out_dir = Some(path.clone());
    cfg.save().map_err(err)?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
fn open_dir(state: State<'_, AppState>) -> Res<()> {
    let dir = state.cfg.lock().unwrap().out_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    std::process::Command::new("explorer").arg(&dir).spawn().map_err(err)?;
    Ok(())
}

#[tauri::command]
fn reveal(path: String) -> Res<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let clean = path.replace('"', "");
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{clean}\""))
            .spawn()
            .map_err(err)?;
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    Ok(())
}

#[tauri::command]
fn set_clipboard_watch(state: State<'_, AppState>, on: bool) -> Res<()> {
    state.clipboard_watch.store(on, Ordering::Relaxed);
    let mut cfg = state.cfg.lock().unwrap();
    cfg.clipboard_watch = on;
    cfg.save().map_err(err)
}

#[tauri::command]
fn download(app: AppHandle, clips: Vec<ClipInfo>) -> usize {
    clips.into_iter().filter(|c| queue(&app, c.clone())).count()
}

#[tauri::command]
fn add_links(app: AppHandle, text: String) -> LinkResult {
    let queued = queue_links(&app, &text, true);
    let profile = if queued == 0 {
        medal::extract_profile(&text)
    } else {
        None
    };
    LinkResult { queued, profile }
}

// ------------------------------------------------------------------ queue & fast download

fn queue_links(app: &AppHandle, text: &str, allow_bare: bool) -> usize {
    medal::extract_ids(text, allow_bare)
        .into_iter()
        .filter(|id| {
            queue(
                app,
                ClipInfo {
                    id: id.clone(),
                    title: "Finding clip…".into(),
                    url: None,
                    preview_url: None,
                    thumb: None,
                    duration: 0.0,
                    created: 0,
                    game: None,
                    saved: false,
                },
            )
        })
        .count()
}

fn queue(app: &AppHandle, clip: ClipInfo) -> bool {
    let state = app.state::<AppState>();
    if !state.active.lock().unwrap().insert(clip.id.clone()) {
        return false;
    }
    let _ = app.emit(
        "dl-queued",
        Queued { id: clip.id.clone(), title: clip.title.clone(), thumb: clip.thumb.clone() },
    );
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let id = clip.id.clone();
        let res = run_download(&app, &state, clip).await;
        state.active.lock().unwrap().remove(&id);
        match res {
            Ok(d) => {
                let _ = app.emit("dl-done", d);
            }
            Err(e) => {
                let _ = app.emit("dl-error", Failed { id, error: format!("{e:#}") });
            }
        }
    });
    true
}

fn parse_content_range(header: &str) -> Option<(u64, u64)> {
    // Format: "bytes 0-2097151/23978355"
    let rest = header.strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (_, end) = range.split_once('-')?;
    Some((end.parse().ok()?, total.parse().ok()?))
}

fn write_all_at(file: &std::fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        while !buf.is_empty() {
            let n = file.seek_write(buf, offset)?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "zero write"));
            }
            buf = &buf[n..];
            offset += n as u64;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)?;
    }
    Ok(())
}

async fn stream_range_into(
    resp: reqwest::Response,
    file: Arc<std::fs::File>,
    mut offset: u64,
    done: Arc<AtomicU64>,
    total: u64,
    id: String,
    app: AppHandle,
    last_emit: Arc<Mutex<Instant>>,
) -> anyhow::Result<()> {
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let len = chunk.len() as u64;
        let f = file.clone();
        let off = offset;
        tokio::task::spawn_blocking(move || write_all_at(&f, &chunk, off)).await??;
        offset += len;
        let cur = done.fetch_add(len, Ordering::Relaxed) + len;
        let should_emit = {
            let mut last = last_emit.lock().unwrap();
            if last.elapsed() >= Duration::from_millis(100) || (total > 0 && cur >= total) {
                *last = Instant::now();
                true
            } else {
                false
            }
        };
        if should_emit {
            let _ = app.emit("dl-progress", Progress { id: id.clone(), done: cur, total });
        }
    }
    Ok(())
}

async fn run_download(app: &AppHandle, state: &AppState, mut clip: ClipInfo) -> anyhow::Result<Done> {
    let (dir, token) = {
        let cfg = state.cfg.lock().unwrap();
        (cfg.out_dir(), cfg.auth.clone())
    };
    tokio::fs::create_dir_all(&dir).await?;

    // Fast skip before hitting network
    if let Some(p) = medal::saved_ids(&dir).get(&clip.id) {
        return Ok(Done { id: clip.id, path: p.display().to_string(), bytes: 0, skipped: true });
    }

    // Resolve metadata concurrently using fast JSON pool (doesn't stall download slots)
    let had_cached_url = clip.url.is_some();
    if clip.url.is_none() {
        let _rpermit = state.resolve_sem.acquire().await;
        let r = medal::resolve(&state.client, &clip.id, token.as_deref()).await?;
        clip.url = r.url;
        clip.title = r.title;
        clip.thumb = clip.thumb.or(r.thumb);
        let _ = app.emit(
            "dl-queued",
            Queued { id: clip.id.clone(), title: clip.title.clone(), thumb: clip.thumb.clone() },
        );
    }

    let _dl_permit = state.dl_sem.acquire().await;

    // Re-check disk in case another job just saved it
    if let Some(p) = medal::saved_ids(&dir).get(&clip.id) {
        return Ok(Done { id: clip.id, path: p.display().to_string(), bytes: 0, skipped: true });
    }

    let mut url = clip.url.clone().unwrap();
    let range_hdr = format!("bytes=0-{}", FIRST_CHUNK_BYTES - 1);
    let mut resp = state.client.get(&url).header(reqwest::header::RANGE, &range_hdr).send().await?;

    if had_cached_url
        && matches!(
            resp.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::GONE
        )
    {
        if let Ok(fresh) = medal::resolve(&state.client, &clip.id, token.as_deref()).await {
            if let Some(fresh_url) = fresh.url {
                url = fresh_url;
                resp = state.client.get(&url).header(reqwest::header::RANGE, &range_hdr).send().await?;
            }
        }
    }

    let path = medal::file_path(&dir, &clip.title, &clip.id);
    let part = path.with_extension("mp4.part");
    let status = resp.status();

    let total_bytes: u64 = if status == StatusCode::PARTIAL_CONTENT {
        let cr = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range)
            .ok_or_else(|| anyhow!("Invalid Content-Range from CDN"))?;
        let (first_end, total) = cr;

        let std_file = {
            let p = part.clone();
            tokio::task::spawn_blocking(move || -> std::io::Result<std::fs::File> {
                let f = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&p)?;
                f.set_len(total)?;
                Ok(f)
            })
            .await??
        };
        let file = Arc::new(std_file);
        let done = Arc::new(AtomicU64::new(0));
        let last_emit = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1)));

        let mut set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        // Chunk 0 (already open)
        set.spawn(stream_range_into(
            resp,
            file.clone(),
            0,
            done.clone(),
            total,
            clip.id.clone(),
            app.clone(),
            last_emit.clone(),
        ));

        // Remaining chunks in parallel (up to MAX_CHUNKS_PER_CLIP total)
        let rem_start = first_end + 1;
        if rem_start < total {
            let remaining = total - rem_start;
            let extra_chunks = ((remaining + FIRST_CHUNK_BYTES - 1) / FIRST_CHUNK_BYTES)
                .min(MAX_CHUNKS_PER_CLIP - 1)
                .max(1);
            let chunk_size = remaining.div_ceil(extra_chunks);
            for i in 0..extra_chunks {
                let start = rem_start + i * chunk_size;
                if start >= total {
                    break;
                }
                let end = (start + chunk_size - 1).min(total - 1);
                let client = state.client.clone();
                let u = url.clone();
                let f = file.clone();
                let d = done.clone();
                let id = clip.id.clone();
                let a = app.clone();
                let le = last_emit.clone();
                set.spawn(async move {
                    let r = client
                        .get(&u)
                        .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                        .send()
                        .await?
                        .error_for_status()?;
                    stream_range_into(r, f, start, d, total, id, a, le).await
                });
            }
        }

        let mut join_err = None;
        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    join_err = Some(e);
                    set.abort_all();
                    break;
                }
                Err(e) => {
                    join_err = Some(e.into());
                    set.abort_all();
                    break;
                }
            }
        }
        drop(file);
        if let Some(e) = join_err {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
        total
    } else {
        // CDN returned 200 OK (no Range support or ignored) — stream sequentially
        let resp = resp.error_for_status()?;
        let total = resp.content_length().unwrap_or(0);
        let std_file = {
            let p = part.clone();
            tokio::task::spawn_blocking(move || std::fs::File::create(&p)).await??
        };
        let file = Arc::new(std_file);
        let done = Arc::new(AtomicU64::new(0));
        let last_emit = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1)));
        let res = stream_range_into(
            resp,
            file.clone(),
            0,
            done.clone(),
            total,
            clip.id.clone(),
            app.clone(),
            last_emit,
        )
        .await;
        drop(file);
        if let Err(e) = res {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
        done.load(Ordering::Relaxed)
    };

    tokio::fs::rename(&part, &path).await?;
    Ok(Done { id: clip.id, path: path.display().to_string(), bytes: total_bytes, skipped: false })
}

// ------------------------------------------------------------------ main

fn start_clipboard_watch(app: AppHandle, on: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let Ok(mut cb) = arboard::Clipboard::new() else { return };
        let mut last = cb.get_text().unwrap_or_default();
        loop {
            std::thread::sleep(Duration::from_millis(350));
            let Ok(t) = cb.get_text() else { continue };
            if t == last {
                continue;
            }
            last = t.clone();
            if on.load(Ordering::Relaxed) {
                let n = queue_links(&app, &t, false);
                if n > 0 {
                    let _ = app.emit("clipboard-grabbed", n);
                }
            }
        }
    });
}

fn main() {
    let cfg = Config::load();
    let watch = Arc::new(AtomicBool::new(cfg.clipboard_watch));
    let client = reqwest::Client::builder()
        .user_agent(medal::UA)
        .connect_timeout(Duration::from_secs(12))
        .pool_max_idle_per_host(24)
        .tcp_nodelay(true)
        .http2_adaptive_window(true)
        .build()
        .expect("http client");

    let watch2 = watch.clone();
    tauri::Builder::default()
        .manage(AppState {
            cfg: Mutex::new(cfg),
            client,
            dl_sem: Arc::new(Semaphore::new(PARALLEL_CLIPS)),
            resolve_sem: Arc::new(Semaphore::new(PARALLEL_RESOLVE)),
            active: Mutex::new(HashSet::new()),
            clipboard_watch: watch,
        })
        .setup(move |app| {
            start_clipboard_watch(app.handle().clone(), watch2);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_clips,
            lookup_profile,
            open_login,
            login_with_token,
            logout,
            pick_dir,
            open_dir,
            reveal,
            set_clipboard_watch,
            download,
            add_links
        ])
        .run(tauri::generate_context!())
        .expect("error while running Medal Clips");
}

#[cfg(test)]
mod tests {
    use super::parse_content_range;

    #[test]
    fn parses_content_range_header() {
        assert_eq!(parse_content_range("bytes 0-2097151/23978355"), Some((2097151, 23978355)));
        assert_eq!(parse_content_range("bytes 0-100/101"), Some((100, 101)));
        assert_eq!(parse_content_range("bytes */100"), None);
    }
}
