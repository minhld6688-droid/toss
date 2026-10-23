//! v0.5.2 short links: `https://<page_host>/<code>` instead of `https://<page_host>/#toss1…`.
//!
//! The link page's `/api/s` (web/api/s.js) stores a signed ticket or invite under a 9-char
//! code. Creating a link asks it for a code (a few seconds at most) and falls back to the
//! long link when the service is unreachable; pasting a short link resolves it back to the
//! ticket, which is then verified exactly like a pasted long link.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

/// Length of a short code (base32 `a-z2-7`, 45 bits).
pub const CODE_LEN: usize = 9;
const FILE: &str = "shortlinks.json";
/// Cached long → short pairs kept on disk (oldest dropped first).
const MAX_CACHE: usize = 400;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize, Default)]
struct Disk {
    /// `[long text, short url]`, oldest first.
    links: Vec<(String, String)>,
}

/// Cache of short links this device made, so a job keeps the same short link.
pub struct ShortLinks {
    path: PathBuf,
    links: Mutex<Vec<(String, String)>>,
    /// Texts already sent this session (avoids retry storms while the service is down).
    tried: Mutex<HashSet<String>>,
}

impl ShortLinks {
    pub fn load(dir: &Path) -> ShortLinks {
        let path = dir.join(FILE);
        let links = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Disk>(&b).ok())
            .map(|d| d.links)
            .unwrap_or_default();
        ShortLinks {
            path,
            links: Mutex::new(links),
            tried: Mutex::new(HashSet::new()),
        }
    }

    /// Cached short url for `text` (a `toss1…` / `tossinv1…` string), if any.
    pub fn get(&self, text: &str) -> Option<String> {
        let links = self.links.lock().unwrap();
        links.iter().rev().find(|(k, _)| k == text).map(|(_, v)| v.clone())
    }

    fn put(&self, text: &str, url: &str) {
        let mut links = self.links.lock().unwrap();
        links.retain(|(k, _)| k != text);
        links.push((text.to_string(), url.to_string()));
        let extra = links.len().saturating_sub(MAX_CACHE);
        links.drain(..extra);
        let disk = Disk {
            links: links.clone(),
        };
        drop(links);
        if let Ok(b) = serde_json::to_vec(&disk) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// Whether `text` was not tried yet this session (and marks it tried).
    pub fn first_try(&self, text: &str) -> bool {
        self.tried.lock().unwrap().insert(text.to_string())
    }

    /// Short url for `text`: cached, or asked from `https://<page_host>/api/s`.
    /// `None` when the service is unreachable or refuses (admin tickets, page hosts
    /// without the API); callers then use the long link.
    pub async fn shorten(&self, page_host: &str, text: &str) -> Option<String> {
        if let Some(u) = self.get(text) {
            return Some(u);
        }
        self.first_try(text);
        match request_short(page_host, text).await {
            Ok(url) => {
                self.put(text, &url);
                Some(url)
            }
            Err(e) => {
                tracing::info!("short link unavailable, using the long link: {e:#}");
                None
            }
        }
    }
}

fn client() -> Result<&'static reqwest::Client> {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = C.get() {
        return Ok(c);
    }
    // Also done in lib.rs `run`; repeated here so tests and other entry points never hit
    // reqwest's "no rustls crypto provider" panic.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let c = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(TIMEOUT)
        .user_agent(concat!("Toss/", env!("CARGO_PKG_VERSION")))
        .build()?;
    Ok(C.get_or_init(|| c))
}

/// `toss-link.vercel.app` from `https://toss-link.vercel.app/`, `None` when it carries a path
/// (e.g. a GitHub Pages project site, which has no API).
fn bare_host(page_host: &str) -> Option<&str> {
    let h = page_host
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    (!h.is_empty() && !h.contains('/') && h.contains('.')).then_some(h)
}

async fn request_short(page_host: &str, text: &str) -> Result<String> {
    let host = bare_host(page_host).ok_or_else(|| anyhow!("page host {page_host:?} has no API"))?;
    #[derive(Deserialize)]
    struct Resp {
        code: String,
    }
    let body = serde_json::json!({ "t": text }).to_string();
    let r = client()?
        .post(format!("https://{host}/api/s"))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await?;
    let status = r.status();
    let bytes = r.bytes().await?;
    if !status.is_success() {
        bail!("{status}: {}", String::from_utf8_lossy(&bytes[..bytes.len().min(200)]));
    }
    let resp: Resp = serde_json::from_slice(&bytes)?;
    if !is_code(&resp.code) {
        bail!("bad code from server");
    }
    // Built from our own page host, not the server's answer.
    Ok(format!("https://{host}/{}", resp.code))
}

fn is_code(s: &str) -> bool {
    s.len() == CODE_LEN && s.bytes().all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

/// Finds a short link in pasted text: `[https://]host/<code>[/]`, anywhere among
/// whitespace-separated words. Returns `(host, code)`.
pub fn find_short(text: &str) -> Option<(String, String)> {
    text.split_whitespace().find_map(|w| {
        let w = w.trim_matches(|c: char| "<>()[]\"'.,;".contains(c));
        let rest = w
            .strip_prefix("https://")
            .or_else(|| w.strip_prefix("http://"))
            .unwrap_or(w);
        let (host, path) = rest.split_once('/')?;
        let code = path.trim_end_matches('/').to_ascii_lowercase();
        let host_ok = host.contains('.')
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':');
        (host_ok && is_code(&code)).then(|| (host.to_ascii_lowercase(), code))
    })
}

/// If `text` contains a short link, fetches the ticket / invite it stands for; otherwise
/// returns `text` unchanged. The result still goes through the normal signature checks.
pub async fn expand(text: &str) -> Result<String> {
    let Some((host, code)) = find_short(text) else {
        return Ok(text.to_string());
    };
    #[derive(Deserialize)]
    struct Resp {
        t: String,
    }
    let r = client()?
        .get(format!("https://{host}/api/s?c={code}"))
        .send()
        .await
        .map_err(|e| anyhow!("Không mở được link rút gọn — kiểm tra mạng ({e})"))?;
    match r.status().as_u16() {
        200 => {}
        404 => bail!("Không tìm thấy link này (gõ thiếu ký tự?)"),
        410 => bail!("Link đã hết hạn"),
        s => bail!("Máy chủ link trả lỗi {s}, thử lại sau"),
    }
    let resp: Resp = r
        .json()
        .await
        .map_err(|_| anyhow!("Máy chủ link trả dữ liệu lạ"))?;
    let t = resp.t.trim().to_ascii_lowercase();
    if !(t.starts_with("toss1") || t.starts_with("tossinv1")) {
        bail!("Link rút gọn không trỏ tới link Toss");
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_short_links() {
        let want = Some(("toss-link.vercel.app".to_string(), "k7q2xa3mp".to_string()));
        assert_eq!(find_short("https://toss-link.vercel.app/k7q2xa3mp"), want);
        assert_eq!(find_short("toss-link.vercel.app/K7Q2XA3MP/"), want);
        assert_eq!(find_short("📁 RAWW · 57 MB — https://toss-link.vercel.app/k7q2xa3mp."), want);
        assert_eq!(find_short("https://toss-link.vercel.app/#toss1abc"), None);
        assert_eq!(find_short("https://toss-link.vercel.app/k7q2xa3m"), None);
        assert_eq!(find_short("https://toss-link.vercel.app/k7q2xa3m1"), None);
        assert_eq!(find_short("toss1abcdefghi"), None);
        assert_eq!(find_short("hello/k7q2xa3mp"), None);
    }

    #[test]
    fn bare_hosts() {
        assert_eq!(bare_host("toss-link.vercel.app"), Some("toss-link.vercel.app"));
        assert_eq!(bare_host("https://toss-link.vercel.app/"), Some("toss-link.vercel.app"));
        assert_eq!(bare_host("minhld6688-droid.github.io/toss"), None);
        assert_eq!(bare_host(""), None);
    }

    #[test]
    fn cache_roundtrip_and_cap() {
        let dir = std::env::temp_dir().join(format!("toss-short-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = ShortLinks::load(&dir);
        s.put("toss1a", "https://h.x/aaaaaaaaa");
        s.put("toss1b", "https://h.x/bbbbbbbbb");
        s.put("toss1a", "https://h.x/ccccccccc");
        let s2 = ShortLinks::load(&dir);
        assert_eq!(s2.get("toss1a").as_deref(), Some("https://h.x/ccccccccc"));
        assert_eq!(s2.get("toss1b").as_deref(), Some("https://h.x/bbbbbbbbb"));
        for i in 0..(MAX_CACHE + 5) {
            s2.put(&format!("k{i}"), "u");
        }
        assert_eq!(s2.links.lock().unwrap().len(), MAX_CACHE);
        assert!(s2.get("toss1a").is_none());
        assert!(s2.first_try("x") && !s2.first_try("x"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn expand_passes_other_text_through() {
        assert_eq!(expand("toss1abc").await.unwrap(), "toss1abc");
    }
}
