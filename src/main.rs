//! medal-dl — super fast Medal.tv clip downloader.
//!
//! * Run with no arguments -> interactive mode: paste links, or just COPY a Medal
//!   link anywhere and it downloads automatically (clipboard watch).
//! * `medal-dl <links...>` / `medal-dl -f links.txt` -> batch download.
//! * `medal-dl login` + `medal-dl mine` -> grab ALL of your own clips.

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use console::style;
use futures_util::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "medal-dl", version, about = "Super fast Medal.tv clip downloader")]
struct Cli {
    /// Clip links / clip IDs / .txt files with links. Leave empty for interactive mode.
    links: Vec<String>,

    /// Read links from a text file (any text works, links are auto-detected)
    #[arg(short, long)]
    file: Option<PathBuf>,

    /// Output folder (default: Videos\Medal Clips, or what you saved with `set-dir`)
    #[arg(short, long, global = true)]
    out: Option<PathBuf>,

    /// How many clips to download at the same time
    #[arg(short = 'j', long, default_value_t = 6, global = true)]
    jobs: usize,

    /// Interactive mode: don't watch the clipboard for copied links
    #[arg(long)]
    no_clipboard: bool,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Save your Medal session so you can bulk-download your own clips
    Login {
        /// Your X-Authentication value ("userId,key"). Prompted if omitted.
        token: Option<String>,
    },
    /// Forget the saved session
    Logout,
    /// Download ALL clips from your account (needs `login` first). Skips ones you already have.
    Mine {
        /// Only grab the newest N clips
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Remember a default output folder
    SetDir { dir: PathBuf },
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
struct Config {
    auth: Option<String>,
    out_dir: Option<PathBuf>,
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("medal-dl")
        .join("config.json")
}

impl Config {
    fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save(&self) -> Result<()> {
        let p = config_path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

fn default_out_dir(cfg: &Config) -> PathBuf {
    if let Some(d) = &cfg.out_dir {
        return d.clone();
    }
    dirs::video_dir()
        .map(|d| d.join("Medal Clips"))
        .unwrap_or_else(|| PathBuf::from("medal-clips"))
}

// ---------------------------------------------------------------------------
// Link parsing
// ---------------------------------------------------------------------------

static LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)medal\.tv/[^\s]*?(?:clips?/|contentId=)([A-Za-z0-9_-]{5,})").unwrap()
});
static BARE_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{8,40}$").unwrap());
static LD_JSON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<script type="application/ld\+json">(.*?)</script>"#).unwrap()
});
static CONTENT_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""contentUrl":"(https://[^"]+)""#).unwrap());
static OG_VIDEO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"property="og:video:url" content="([^"]+)""#).unwrap()
});
static EXISTING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([A-Za-z0-9_-]+)\]\.mp4$").unwrap());

/// Pull every Medal clip id out of arbitrary text. If `allow_bare`, a lone token
/// that looks like a clip id is accepted too.
fn extract_ids(text: &str, allow_bare: bool) -> Vec<String> {
    let mut out: Vec<String> = LINK_RE
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .collect();
    if allow_bare {
        for tok in text.split_whitespace() {
            if BARE_ID_RE.is_match(tok) {
                out.push(tok.to_string());
            }
        }
    }
    let mut seen = HashSet::new();
    out.retain(|id| seen.insert(id.clone()));
    out
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c))
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(100).collect();
    let s = s.trim_end_matches(['.', ' ']).trim().to_string();
    if s.is_empty() { "clip".into() } else { s }
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("\\u0026", "&")
}

// ---------------------------------------------------------------------------
// Downloader
// ---------------------------------------------------------------------------

struct Clip {
    id: String,
    title: String,
    url: String,
}

enum Outcome {
    Done(PathBuf, u64),
    Skipped,
}

struct Downloader {
    client: reqwest::Client,
    dir: PathBuf,
    sem: Semaphore,
    mp: MultiProgress,
    /// ids already on disk or already queued this session
    seen: Mutex<HashSet<String>>,
}

impl Downloader {
    fn new(dir: PathBuf, jobs: usize) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("can't create output folder {}", dir.display()))?;
        let mut seen = HashSet::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if let Some(c) = EXISTING_RE.captures(&e.file_name().to_string_lossy()) {
                    seen.insert(c[1].to_string());
                }
            }
        }
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .connect_timeout(Duration::from_secs(15))
            .pool_max_idle_per_host(jobs.max(1))
            .build()?;
        Ok(Arc::new(Self {
            client,
            dir,
            sem: Semaphore::new(jobs.max(1)),
            mp: MultiProgress::new(),
            seen: Mutex::new(seen),
        }))
    }

    fn println(&self, msg: impl AsRef<str>) {
        if self.mp.is_hidden() {
            println!("{}", msg.as_ref());
        } else {
            let _ = self.mp.println(msg.as_ref());
        }
    }

    /// Returns false if the clip is already downloaded/queued.
    fn claim(&self, id: &str) -> bool {
        self.seen.lock().unwrap().insert(id.to_string())
    }

    /// Queue a clip. `known` lets bulk mode skip the page scrape.
    fn spawn(self: &Arc<Self>, set: &mut JoinSet<bool>, id: String, known: Option<Clip>) {
        if !self.claim(&id) {
            self.println(format!("{} {id} (already have it)", style("[skip]").dim()));
            return;
        }
        let me = self.clone();
        set.spawn(async move {
            let _permit = me.sem.acquire().await.unwrap();
            let res = async {
                let clip = match known {
                    Some(c) => c,
                    None => me.resolve(&id).await?,
                };
                me.download(&clip).await.map(|o| (clip, o))
            }
            .await;
            match res {
                Ok((clip, Outcome::Done(path, bytes))) => {
                    me.println(format!(
                        "{} {}  {}",
                        style("[done]").green().bold(),
                        clip.title,
                        style(format!(
                            "{:.1} MB -> {}",
                            bytes as f64 / 1_048_576.0,
                            path.file_name().unwrap().to_string_lossy()
                        ))
                        .dim()
                    ));
                    true
                }
                Ok((clip, Outcome::Skipped)) => {
                    me.println(format!("{} {} (already have it)", style("[skip]").dim(), clip.title));
                    true
                }
                Err(e) => {
                    me.seen.lock().unwrap().remove(&id); // allow retry
                    me.println(format!("{} {id}: {e:#}", style("[fail]").red().bold()));
                    false
                }
            }
        });
    }

    /// Scrape the public clip page for the raw mp4 url.
    async fn resolve(&self, id: &str) -> Result<Clip> {
        let page_url = format!("https://medal.tv/clips/{id}");
        let resp = self.client.get(&page_url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            bail!("clip not found (deleted or private?)");
        }
        let html = resp.error_for_status()?.text().await?;

        // 1) JSON-LD VideoObject (cleanest, always the clip itself)
        for cap in LD_JSON_RE.captures_iter(&html) {
            if let Ok(v) = serde_json::from_str::<Value>(&cap[1]) {
                if let Some(url) = v.get("contentUrl").and_then(Value::as_str) {
                    let title = v.get("name").and_then(Value::as_str).unwrap_or("clip");
                    return Ok(Clip {
                        id: id.into(),
                        title: title.to_string(),
                        url: html_unescape(url),
                    });
                }
            }
        }
        // 2) hydration data / 3) og:video fallback
        let url = CONTENT_URL_RE
            .captures(&html)
            .or_else(|| OG_VIDEO_RE.captures(&html))
            .map(|c| html_unescape(&c[1]))
            .ok_or_else(|| anyhow!("no video found on page (private clip?)"))?;
        Ok(Clip { id: id.into(), title: "clip".into(), url })
    }

    async fn download(&self, clip: &Clip) -> Result<Outcome> {
        let path = self
            .dir
            .join(format!("{} [{}].mp4", sanitize(&clip.title), clip.id));
        if path.exists() {
            return Ok(Outcome::Skipped);
        }
        let part = path.with_extension("mp4.part");

        let resp = self.client.get(&clip.url).send().await?.error_for_status()?;
        let total = resp.content_length().unwrap_or(0);

        let pb = self.mp.add(ProgressBar::new(total));
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.cyan} {msg:32!} [{bar:28.cyan/blue}] {bytes:>9}/{total_bytes:<9} {bytes_per_sec:>11}",
            )
            .unwrap()
            .progress_chars("=> "),
        );
        pb.set_message(clip.title.clone());
        pb.enable_steady_tick(Duration::from_millis(120));

        let result: Result<u64> = async {
            let mut f = tokio::io::BufWriter::with_capacity(
                1 << 20,
                tokio::fs::File::create(&part).await?,
            );
            let mut stream = resp.bytes_stream();
            let mut n = 0u64;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                f.write_all(&chunk).await?;
                n += chunk.len() as u64;
                pb.set_position(n);
            }
            f.flush().await?;
            Ok(n)
        }
        .await;
        pb.finish_and_clear();
        self.mp.remove(&pb);

        match result {
            Ok(n) => {
                tokio::fs::rename(&part, &path).await?;
                Ok(Outcome::Done(path, n))
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                Err(e)
            }
        }
    }
}

async fn wait_all(set: &mut JoinSet<bool>) -> (usize, usize) {
    let (mut ok, mut fail) = (0, 0);
    while let Some(r) = set.join_next().await {
        if matches!(r, Ok(true)) { ok += 1 } else { fail += 1 }
    }
    (ok, fail)
}

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

enum Input {
    Typed(String),
    Clipboard(String),
    Eof,
}

async fn interactive(dl: Arc<Downloader>, clipboard: bool) -> Result<()> {
    println!("{}", style("medal-dl — super fast Medal clip downloader").cyan().bold());
    println!("Saving to: {}", style(dl.dir.display()).yellow());
    if clipboard {
        println!("• {} any Medal clip link (Ctrl+C in Discord/browser) and it downloads instantly", style("Copy").green().bold());
    }
    println!("• Or paste links here and press Enter (multiple links at once is fine)");
    println!("• Type {} to finish & quit\n", style("q").bold());

    let (tx, mut rx) = mpsc::unbounded_channel::<Input>();

    let tx_in = tx.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => { if tx_in.send(Input::Typed(l)).is_err() { return; } }
                Err(_) => break,
            }
        }
        let _ = tx_in.send(Input::Eof);
    });

    if clipboard {
        let tx_cb = tx.clone();
        std::thread::spawn(move || {
            let Ok(mut cb) = arboard::Clipboard::new() else { return };
            // ignore whatever was on the clipboard before we started
            let mut last = cb.get_text().unwrap_or_default();
            loop {
                std::thread::sleep(Duration::from_millis(300));
                if let Ok(t) = cb.get_text() {
                    if t != last {
                        last = t.clone();
                        if tx_cb.send(Input::Clipboard(t)).is_err() { return; }
                    }
                }
            }
        });
    }
    drop(tx);

    let mut set = JoinSet::new();
    while let Some(input) = rx.recv().await {
        // reap finished tasks so the set doesn't grow forever
        while set.try_join_next().is_some() {}
        let (text, typed) = match input {
            Input::Typed(t) => (t, true),
            Input::Clipboard(t) => (t, false),
            Input::Eof => break,
        };
        let trimmed = text.trim();
        if typed && matches!(trimmed.to_lowercase().as_str(), "q" | "quit" | "exit") {
            break;
        }
        let ids = extract_ids(trimmed, typed);
        if ids.is_empty() {
            if typed && !trimmed.is_empty() {
                dl.println(format!("{} no Medal link found in that", style("[?]").yellow()));
            }
            continue;
        }
        for id in ids {
            if !typed {
                dl.println(format!("{} {id}", style("[clipboard]").magenta()));
            }
            dl.spawn(&mut set, id, None);
        }
    }

    if !set.is_empty() {
        dl.println("Finishing remaining downloads...");
    }
    wait_all(&mut set).await;
    println!("Bye! Clips are in {}", dl.dir.display());
    Ok(())
}

async fn batch(dl: Arc<Downloader>, ids: Vec<String>) -> Result<()> {
    println!("Downloading {} clip(s) to {}", ids.len(), style(dl.dir.display()).yellow());
    let mut set = JoinSet::new();
    for id in ids {
        dl.spawn(&mut set, id, None);
    }
    let (ok, fail) = wait_all(&mut set).await;
    println!("\n{} ok, {} failed", style(ok).green().bold(), style(fail).red().bold());
    if fail > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn parse_token(raw: &str) -> Result<(String, String)> {
    let t = raw.trim().trim_matches('"');
    let t = t
        .strip_prefix("X-Authentication:")
        .or_else(|| t.strip_prefix("x-authentication:"))
        .unwrap_or(t)
        .trim();
    let (uid, key) = t
        .split_once(',')
        .ok_or_else(|| anyhow!("token should look like  12345678,abcdef0123..."))?;
    let uid = uid.trim();
    if uid.is_empty() || !uid.chars().all(|c| c.is_ascii_digit()) {
        bail!("token should start with your numeric user id, like  12345678,abcdef...");
    }
    Ok((uid.to_string(), key.trim().to_string()))
}

async fn fetch_page(client: &reqwest::Client, token: &str, uid: &str, offset: usize, limit: usize) -> Result<Vec<Value>> {
    let url = format!(
        "https://medal.tv/api/content?userId={uid}&limit={limit}&offset={offset}&sortBy=publishedAt&sortDirection=DESC"
    );
    let resp = client.get(&url).header("X-Authentication", token).send().await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED || resp.status() == reqwest::StatusCode::FORBIDDEN {
        bail!("Medal rejected your session (expired?). Run `medal-dl login` again.");
    }
    let v: Value = resp.error_for_status()?.json_or_text().await?;
    match v {
        Value::Array(a) => Ok(a),
        other => bail!("unexpected API response: {}", other.to_string().chars().take(200).collect::<String>()),
    }
}

trait JsonOrText {
    async fn json_or_text(self) -> Result<Value>;
}
impl JsonOrText for reqwest::Response {
    async fn json_or_text(self) -> Result<Value> {
        let t = self.text().await?;
        serde_json::from_str(&t).with_context(|| format!("bad JSON: {}", t.chars().take(200).collect::<String>()))
    }
}

async fn login(cfg: &mut Config, token: Option<String>) -> Result<()> {
    let raw = match token {
        Some(t) => t,
        None => {
            println!("{}", style("Log in to medal-dl").cyan().bold());
            println!("1. Open https://medal.tv in your browser and make sure you're logged in");
            println!("2. Press F12 -> Network tab, then refresh the page (F5)");
            println!("3. Type  api  in the filter box and click any request to medal.tv/api/...");
            println!("4. Under Request Headers find  X-Authentication  and copy its value");
            println!("   (looks like  12345678,a1b2c3d4e5...)\n");
            print!("Paste it here: ");
            std::io::stdout().flush()?;
            let mut s = String::new();
            std::io::stdin().read_line(&mut s)?;
            s
        }
    };
    let (uid, key) = parse_token(&raw)?;
    let token = format!("{uid},{key}");
    let client = reqwest::Client::builder().user_agent(UA).build()?;
    print!("Checking... ");
    std::io::stdout().flush()?;
    fetch_page(&client, &token, &uid, 0, 1).await?;
    println!("{}", style("logged in!").green().bold());
    cfg.auth = Some(token);
    cfg.save()?;
    println!("Saved. Now run:  medal-dl mine");
    Ok(())
}

async fn mine(dl: Arc<Downloader>, cfg: &Config, limit: Option<usize>) -> Result<()> {
    let token = cfg
        .auth
        .clone()
        .ok_or_else(|| anyhow!("not logged in — run `medal-dl login` first"))?;
    let (uid, _) = parse_token(&token)?;
    println!("Fetching your clip list...");

    let page = 100;
    let mut offset = 0;
    let mut items = Vec::new();
    loop {
        let batch = fetch_page(&dl.client, &token, &uid, offset, page).await?;
        let n = batch.len();
        items.extend(batch);
        offset += n;
        if n == 0 || limit.is_some_and(|l| items.len() >= l) {
            break;
        }
    }
    if let Some(l) = limit {
        items.truncate(l);
    }
    println!("Found {} clip(s). Saving to {}", items.len(), style(dl.dir.display()).yellow());

    let mut set = JoinSet::new();
    for it in items {
        let Some(id) = it.get("contentId").and_then(|v| v.as_str().map(String::from).or_else(|| Some(v.to_string()))) else { continue };
        let id = id.trim_matches('"').to_string();
        let known = it.get("contentUrl").and_then(Value::as_str).map(|u| Clip {
            id: id.clone(),
            title: it.get("contentTitle").and_then(Value::as_str).unwrap_or("clip").to_string(),
            url: u.to_string(),
        });
        dl.spawn(&mut set, id, known);
    }
    let (ok, fail) = wait_all(&mut set).await;
    println!("\n{} ok, {} failed", style(ok).green().bold(), style(fail).red().bold());
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{} {e:#}", style("error:").red().bold());
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = Config::load();
    let out = cli.out.clone().unwrap_or_else(|| default_out_dir(&cfg));

    match cli.cmd {
        Some(Cmd::Login { token }) => return login(&mut cfg, token).await,
        Some(Cmd::Logout) => {
            cfg.auth = None;
            cfg.save()?;
            println!("Logged out.");
            return Ok(());
        }
        Some(Cmd::SetDir { dir }) => {
            let dir = std::path::absolute(&dir).unwrap_or(dir);
            cfg.out_dir = Some(dir.clone());
            cfg.save()?;
            println!("Clips will now be saved to {}", dir.display());
            return Ok(());
        }
        Some(Cmd::Mine { limit }) => {
            let dl = Downloader::new(out, cli.jobs)?;
            return mine(dl, &cfg, limit).await;
        }
        None => {}
    }

    // Collect ids from args / files
    let mut text = String::new();
    for a in &cli.links {
        let p = Path::new(a);
        if p.is_file() {
            text.push_str(&std::fs::read_to_string(p)?);
        } else {
            text.push_str(a);
        }
        text.push('\n');
    }
    if let Some(f) = &cli.file {
        text.push_str(&std::fs::read_to_string(f).with_context(|| format!("can't read {}", f.display()))?);
    }

    let dl = Downloader::new(out, cli.jobs)?;
    if text.trim().is_empty() {
        interactive(dl, !cli.no_clipboard).await
    } else {
        let ids = extract_ids(&text, true);
        if ids.is_empty() {
            bail!("no Medal links found");
        }
        batch(dl, ids).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(extract_ids("https://medal.tv/games/valorant/clips/abcDEF123/xyz?invite=1", false), vec!["abcDEF123"]);
        assert_eq!(extract_ids("medal.tv/clips/lrtUkuknxOrOSZawI", false), vec!["lrtUkuknxOrOSZawI"]);
        assert_eq!(extract_ids("https://medal.tv/?contentId=abc12345", false), vec!["abc12345"]);
        assert_eq!(extract_ids("lrtUkuknxOrOSZawI", true), vec!["lrtUkuknxOrOSZawI"]);
        assert!(extract_ids("lrtUkuknxOrOSZawI", false).is_empty());
        assert!(extract_ids("hello world", true).is_empty());
        assert_eq!(extract_ids("medal.tv/clips/aaaaaaaaa1 bbbbbbbbb2", true), vec!["aaaaaaaaa1", "bbbbbbbbb2"]);
    }

    #[test]
    fn names() {
        assert_eq!(sanitize("a/b:c*?"), "abc");
        assert_eq!(sanitize("   "), "clip");
    }
}
