//! Medal.tv helpers: link/profile parsing, fast JSON API, page fallback, account listing.

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

// ---------------------------------------------------------------- config

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Config {
    pub auth: Option<String>,
    pub out_dir: Option<PathBuf>,
    #[serde(default)]
    pub clipboard_watch: bool,
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("medal-dl")
        .join("config.json")
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<()> {
        let p = config_path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
    pub fn out_dir(&self) -> PathBuf {
        self.out_dir.clone().unwrap_or_else(|| {
            dirs::video_dir()
                .map(|d| d.join("Medal Clips"))
                .unwrap_or_else(|| PathBuf::from("medal-clips"))
        })
    }
    pub fn user_id(&self) -> Option<String> {
        self.auth.as_deref().and_then(|t| t.split_once(',')).map(|(u, _)| u.to_string())
    }
}

// ---------------------------------------------------------------- parsing

static LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)medal\.tv/[^\s]*?(?:clips?/|contentId=)([A-Za-z0-9_-]{5,})").unwrap()
});
static PROFILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:https?://)?(?:www\.)?medal\.tv/(?:u|users)/([A-Za-z0-9_.-]+)").unwrap()
});
static BARE_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{10,40}$").unwrap());
static LD_JSON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<script type="application/ld\+json">(.*?)</script>"#).unwrap()
});
static CONTENT_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""contentUrl":"(https://[^"]+)""#).unwrap());
static EXISTING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([A-Za-z0-9_-]+)\]\.mp4$").unwrap());

pub fn extract_ids(text: &str, allow_bare: bool) -> Vec<String> {
    let mut out: Vec<String> = LINK_RE.captures_iter(text).map(|c| c[1].to_string()).collect();
    if allow_bare {
        out.extend(
            text.split_whitespace()
                .filter(|t| !t.contains("medal.tv/u/") && !t.contains("medal.tv/users/"))
                .filter(|t| BARE_ID_RE.is_match(t))
                .map(String::from),
        );
    }
    let mut seen = HashSet::new();
    out.retain(|id| seen.insert(id.clone()));
    out
}

pub fn extract_profile(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !extract_ids(trimmed, false).is_empty() {
        return None;
    }
    PROFILE_RE.captures(trimmed).map(|c| c[1].to_string())
}

pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c))
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(100).collect();
    let s = s.trim_end_matches(['.', ' ']).trim().to_string();
    if s.is_empty() { "clip".into() } else { s }
}

pub fn file_path(dir: &Path, title: &str, id: &str) -> PathBuf {
    dir.join(format!("{} [{}].mp4", sanitize(title), id))
}

/// ids of clips already present in `dir` -> their file path
pub fn saved_ids(dir: &Path) -> std::collections::HashMap<String, PathBuf> {
    let mut m = std::collections::HashMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Some(c) = EXISTING_RE.captures(&e.file_name().to_string_lossy()) {
                m.insert(c[1].to_string(), e.path());
            }
        }
    }
    m
}

// ---------------------------------------------------------------- clips

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClipInfo {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub preview_url: Option<String>,
    pub thumb: Option<String>,
    pub duration: f64,
    pub created: i64,
    pub game: Option<String>,
    #[serde(default)]
    pub saved: bool,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

impl ClipInfo {
    pub fn from_api(v: &Value) -> Option<Self> {
        let id = match v.get("contentId")? {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let url = s(v, "contentUrl");
        let preview_url = [
            "contentPreview480p",
            "contentUrl480p",
            "contentPreview360p",
            "contentUrl360p",
            "contentPreview720p",
        ]
        .iter()
        .find_map(|k| s(v, k))
        .or_else(|| url.clone());
        let thumb = ["thumbnail480p", "thumbnail360p", "thumbnail720p", "thumbnailUrl", "thumbnail1080p"]
            .iter()
            .find_map(|k| s(v, k));
        let game = v
            .get("category")
            .and_then(|c| s(c, "categoryName"))
            .or_else(|| s(v, "categoryName"));
        Some(Self {
            title: s(v, "contentTitle").unwrap_or_else(|| "Untitled clip".into()),
            url,
            preview_url,
            thumb,
            duration: v.get("videoLengthSeconds").and_then(Value::as_f64).unwrap_or(0.0),
            created: ["created", "publishedAt", "contentUploadedAt"]
                .iter()
                .find_map(|k| v.get(*k).and_then(Value::as_i64))
                .unwrap_or(0),
            game,
            saved: false,
            id,
        })
    }
}

/// Fast JSON API lookup first (~20 KB vs ~380 KB HTML), falling back to page scrape.
pub async fn resolve(client: &reqwest::Client, id: &str, token: Option<&str>) -> Result<ClipInfo> {
    let mut req = client.get(format!("https://medal.tv/api/content/{id}"));
    if let Some(t) = token {
        req = req.header("X-Authentication", t);
    }
    if let Ok(resp) = req.send().await {
        if resp.status().is_success() {
            if let Ok(v) = resp.json::<Value>().await {
                if let Some(clip) = ClipInfo::from_api(&v) {
                    if clip.url.is_some() {
                        return Ok(clip);
                    }
                }
            }
        }
    }

    // Fallback: scrape public clip page
    let resp = client.get(format!("https://medal.tv/clips/{id}")).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("Clip not found. It may be deleted or private.");
    }
    let html = resp.error_for_status()?.text().await?;
    for cap in LD_JSON_RE.captures_iter(&html) {
        if let Ok(v) = serde_json::from_str::<Value>(&cap[1]) {
            if let Some(url) = v.get("contentUrl").and_then(Value::as_str) {
                let thumb = match v.get("thumbnailUrl") {
                    Some(Value::Array(a)) => a.first().and_then(Value::as_str).map(String::from),
                    Some(Value::String(s)) => Some(s.clone()),
                    _ => None,
                };
                return Ok(ClipInfo {
                    id: id.into(),
                    title: s(&v, "name").unwrap_or_else(|| "clip".into()),
                    url: Some(url.replace("&amp;", "&")),
                    preview_url: None,
                    thumb,
                    duration: 0.0,
                    created: 0,
                    game: None,
                    saved: false,
                });
            }
        }
    }
    let url = CONTENT_URL_RE
        .captures(&html)
        .map(|c| c[1].replace("\\u0026", "&"))
        .ok_or_else(|| anyhow!("No video on that page. Private clip?"))?;
    Ok(ClipInfo {
        id: id.into(),
        title: "clip".into(),
        url: Some(url),
        preview_url: None,
        thumb: None,
        duration: 0.0,
        created: 0,
        game: None,
        saved: false,
    })
}

// ---------------------------------------------------------------- account & profiles

pub async fn list_clips(
    client: &reqwest::Client,
    token: &str,
    target_user_id: &str,
    offset: usize,
    limit: usize,
) -> Result<Vec<ClipInfo>> {
    let url = format!(
        "https://medal.tv/api/content?userId={target_user_id}&limit={limit}&offset={offset}&sortBy=publishedAt&sortDirection=DESC"
    );
    let resp = client.get(&url).header("X-Authentication", token).send().await?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!("SESSION_EXPIRED");
    }
    let text = resp.text().await?;
    if !status.is_success() {
        bail!("Medal returned {status}: {}", text.chars().take(160).collect::<String>());
    }
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("Unexpected reply: {}", text.chars().take(160).collect::<String>()))?;
    let arr = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("contentObjects")
            .or_else(|| o.get("content"))
            .or_else(|| o.get("items"))
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| anyhow!("Unexpected reply: {}", text.chars().take(160).collect::<String>()))?,
        _ => bail!("Unexpected reply from Medal"),
    };
    Ok(arr.iter().filter_map(ClipInfo::from_api).collect())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfileLookup {
    pub user_id: String,
    pub username: String,
    pub initial_clips: Vec<ClipInfo>,
}

/// Resolve a public profile `medal.tv/u/{username}` to its numeric `userId` and optional SSR clips.
pub async fn lookup_profile(
    client: &reqwest::Client,
    username: &str,
    with_ssr_clips: bool,
) -> Result<ProfileLookup> {
    let resp = client
        .get(format!("https://medal.tv/u/{username}"))
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("Profile @{username} was not found.");
    }
    let html = resp.error_for_status()?.text().await?.replace("\\\"", "\"");

    let user_re = Regex::new(&format!(
        r#"(?i)"userId":"(\d+)","userName":"{}""#,
        regex::escape(username)
    ))?;
    let user_id = user_re
        .captures(&html)
        .map(|c| c[1].to_string())
        .ok_or_else(|| anyhow!("Couldn't find Medal user @{username}."))?;

    let mut initial_clips = Vec::new();
    if with_ssr_clips {
        let mut seen = HashSet::new();
        let clip_id_re = Regex::new(r#""contentId":"([A-Za-z0-9_-]{6,})""#)?;
        let ids: Vec<String> = clip_id_re
            .captures_iter(&html)
            .map(|c| c[1].to_string())
            .filter(|cid| seen.insert(cid.clone()))
            .take(18)
            .collect();
        let futs = ids.iter().map(|cid| resolve(client, cid, None));
        for res in futures_util::future::join_all(futs).await {
            if let Ok(clip) = res {
                initial_clips.push(clip);
            }
        }
    }

    Ok(ProfileLookup {
        user_id,
        username: username.to_string(),
        initial_clips,
    })
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub name: Option<String>,
    pub avatar: Option<String>,
}

pub async fn user_info(client: &reqwest::Client, token: &str) -> UserInfo {
    let uid = token.split(',').next().unwrap_or_default();
    let Ok(resp) = client
        .get(format!("https://medal.tv/api/users/{uid}"))
        .header("X-Authentication", token)
        .send()
        .await
    else {
        return UserInfo::default();
    };
    let Ok(v) = resp.json::<Value>().await else { return UserInfo::default() };
    let v = if v.is_array() { v.get(0).cloned().unwrap_or_default() } else { v };
    UserInfo {
        name: s(&v, "displayName").or_else(|| s(&v, "userName")).or_else(|| s(&v, "username")),
        avatar: s(&v, "thumbnail"),
    }
}

pub fn parse_token(raw: &str) -> Result<String> {
    let t = raw.trim().trim_matches('"');
    let t = t
        .split_once(':')
        .filter(|(k, _)| k.trim().eq_ignore_ascii_case("x-authentication"))
        .map(|(_, v)| v)
        .unwrap_or(t)
        .trim();
    let (uid, key) = t
        .split_once(',')
        .ok_or_else(|| anyhow!("That doesn't look right. It should look like 12345678,abcdef…"))?;
    let uid = uid.trim();
    if uid.is_empty() || !uid.chars().all(|c| c.is_ascii_digit()) {
        bail!("It should start with your numeric user id, like 12345678,abcdef…");
    }
    Ok(format!("{uid},{}", key.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_links_and_bare_ids() {
        assert_eq!(
            extract_ids("https://medal.tv/games/valorant/clips/lrtUkuknxOrOSZawI/xyz?invite=1", false),
            vec!["lrtUkuknxOrOSZawI"]
        );
        assert_eq!(
            extract_ids("https://medal.tv/?contentId=abc1234567", false),
            vec!["abc1234567"]
        );
        assert_eq!(extract_ids("lrtUkuknxOrOSZawI", true), vec!["lrtUkuknxOrOSZawI"]);
        assert!(extract_ids("lrtUkuknxOrOSZawI", false).is_empty());
    }

    #[test]
    fn parses_profiles() {
        assert_eq!(extract_profile("https://medal.tv/u/Medal"), Some("Medal".into()));
        assert_eq!(extract_profile("medal.tv/users/tenz?tab=clips"), Some("tenz".into()));
        assert_eq!(extract_profile("https://medal.tv/u/Medal/clips/abc12345"), None);
    }

    #[test]
    fn sanitizes_filenames() {
        assert_eq!(sanitize("clip: 1/2? *yes*"), "clip 12 yes");
        assert_eq!(sanitize("...   "), "clip");
    }

    #[test]
    fn parses_tokens() {
        assert_eq!(parse_token("X-Authentication: 12345,abc_def").unwrap(), "12345,abc_def");
        assert!(parse_token("not_digits,abc").is_err());
    }

    #[test]
    fn parses_api_clip() {
        let v = json!({
            "contentId": "abc123",
            "contentTitle": "Nice 4k",
            "contentUrl": "https://cdn.medal.tv/1080.mp4",
            "contentPreview480p": "https://cdn.medal.tv/480.mp4",
            "thumbnail480p": "https://cdn.medal.tv/thumb.jpg",
            "videoLengthSeconds": 18.5,
            "created": 1762293254376i64,
            "category": { "categoryName": "Valorant" }
        });
        let c = ClipInfo::from_api(&v).unwrap();
        assert_eq!(c.id, "abc123");
        assert_eq!(c.url.as_deref(), Some("https://cdn.medal.tv/1080.mp4"));
        assert_eq!(c.preview_url.as_deref(), Some("https://cdn.medal.tv/480.mp4"));
        assert_eq!(c.game.as_deref(), Some("Valorant"));
    }
}
