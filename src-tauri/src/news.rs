//! «Новости»: Minecraft: Java Edition news and patch notes from Mojang's launcher feed
//! (`launchercontent.mojang.com`, the one the official launcher shows).
//!
//! The feed and cover images are cached in `.minecraft/signal-cache/` so the tab works offline
//! and images load through the asset protocol (no remote hosts in the CSP).

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::game;

const BASE: &str = "https://launchercontent.mojang.com";
const NEWS: &str = "https://launchercontent.mojang.com/v2/news.json";
const PATCH_NOTES: &str = "https://launchercontent.mojang.com/v2/javaPatchNotes.json";
/// Items shown (and images kept).
const LIMIT: usize = 18;
/// Fresh enough: no network call within this time.
const MAX_AGE_MS: i64 = 30 * 60 * 1000;

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    pub id: String,
    /// `news`, `release`, `snapshot`
    pub kind: String,
    pub title: String,
    pub text: String,
    /// `YYYY-MM-DD`
    pub date: String,
    /// Local cached image, if any.
    pub image: Option<String>,
    /// minecraft.net article (news) …
    pub link: Option<String>,
    /// … or the patch notes body to fetch with `article`.
    pub content_path: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFeed {
    pub items: Vec<NewsItem>,
    pub updated_at: i64,
    /// Shown from the cache because Mojang was unreachable.
    #[serde(default)]
    pub offline: bool,
}

fn cache_dir() -> PathBuf {
    game::minecraft_dir().join("signal-cache")
}

/// The feed, from cache when fresh (`force` skips that), else from Mojang; falls back to the
/// cache when offline.
pub async fn feed(force: bool) -> Result<NewsFeed, String> {
    let cache_file = cache_dir().join("news.json");
    let cached: Option<NewsFeed> = fs::read(&cache_file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(c) = &cached {
        if !force && game::now_ms() - c.updated_at < MAX_AGE_MS {
            return Ok(NewsFeed { items: c.items.clone(), updated_at: c.updated_at, offline: false });
        }
    }
    match fetch().await {
        Ok(items) => {
            let feed = NewsFeed { items, updated_at: game::now_ms(), offline: false };
            let _ = fs::create_dir_all(cache_dir());
            let _ = fs::write(&cache_file, serde_json::to_vec(&feed).unwrap_or_default());
            Ok(feed)
        }
        Err(e) => match cached {
            Some(c) => Ok(NewsFeed { offline: true, ..c }),
            None => Err(format!("Новости не загрузились: {e}")),
        },
    }
}

async fn fetch() -> Result<Vec<NewsItem>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("SignalLauncher/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let news = game::get_json(&client, NEWS).await?;
    let notes = game::get_json(&client, PATCH_NOTES).await?;
    let mut items = parse(&news, &notes);
    items.truncate(LIMIT);

    // Cover images, cached by item id; already downloaded ones are reused.
    let dir = cache_dir().join("news");
    let _ = fs::create_dir_all(&dir);
    for item in &mut items {
        let Some(url) = item.image.take() else { continue };
        let ext = url.rsplit('.').next().filter(|e| ["jpg", "jpeg", "png", "webp"].contains(e)).unwrap_or("jpg");
        let path = dir.join(format!("{}.{ext}", safe_id(&item.id)));
        if !path.is_file() {
            let Ok(response) = client.get(&url).send().await else { continue };
            let Ok(bytes) = response.bytes().await else { continue };
            if fs::write(&path, &bytes).is_err() {
                continue;
            }
        }
        item.image = Some(game::path_str(&path));
    }
    // Drop images of items that left the feed.
    if let Ok(rd) = fs::read_dir(&dir) {
        let keep: Vec<String> = items.iter().map(|i| safe_id(&i.id)).collect();
        for e in rd.flatten() {
            let stem = e.path().file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
            if !keep.contains(&stem) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    Ok(items)
}

/// Java news (`newsType` contains "Java") plus patch notes, newest first. `image` holds the
/// remote URL here; `fetch` swaps it for the cached file.
fn parse(news: &Value, notes: &Value) -> Vec<NewsItem> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let mut items: Vec<NewsItem> = Vec::new();
    for e in news.get("entries").and_then(Value::as_array).into_iter().flatten() {
        let java = e.get("newsType").and_then(Value::as_array).is_some_and(|t| t.iter().any(|x| x.as_str() == Some("Java")));
        if !java {
            continue;
        }
        let image = e.pointer("/newsPageImage/url").or_else(|| e.pointer("/playPageImage/url")).and_then(Value::as_str);
        let link = text(e, "readMoreLink");
        items.push(NewsItem {
            id: text(e, "id"),
            kind: "news".into(),
            title: text(e, "title"),
            text: text(e, "text"),
            date: text(e, "date").chars().take(10).collect(),
            image: image.map(|u| format!("{BASE}{u}")),
            link: allowed_link(&link).then_some(link),
            content_path: None,
            version: None,
        });
    }
    for e in notes.get("entries").and_then(Value::as_array).into_iter().flatten() {
        let kind = if text(e, "type") == "release" { "release" } else { "snapshot" };
        items.push(NewsItem {
            id: text(e, "id"),
            kind: kind.into(),
            title: text(e, "title"),
            text: text(e, "shortText"),
            date: text(e, "date").chars().take(10).collect(),
            image: e.pointer("/image/url").and_then(Value::as_str).map(|u| format!("{BASE}{u}")),
            link: None,
            content_path: Some(text(e, "contentPath")).filter(|p| safe_content_path(p)),
            version: Some(text(e, "version")).filter(|v| !v.is_empty()),
        });
    }
    items.retain(|i| !i.id.is_empty() && !i.title.is_empty());
    items.sort_by(|a, b| b.date.cmp(&a.date));
    items
}

/// Full patch notes (HTML from Mojang; the UI shows it through a tag allowlist).
pub async fn article(content_path: &str) -> Result<String, String> {
    if !safe_content_path(content_path) {
        return Err("Некорректный адрес статьи".into());
    }
    let client = game::http_client()?;
    let json = game::get_json(&client, &format!("{BASE}/v2/{content_path}")).await?;
    json.get("body").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "В статье нет текста".into())
}

/// `javaPatchNotes/<hash>.json`
fn safe_content_path(p: &str) -> bool {
    p.starts_with("javaPatchNotes/") && p.ends_with(".json") && !p.contains("..") && p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
}

/// Links the launcher opens in the browser.
pub fn allowed_link(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    ["www.minecraft.net", "minecraft.net", "feedback.minecraft.net", "help.minecraft.net", "modrinth.com", "www.curseforge.com", "curseforge.com", "console.curseforge.com"].contains(&host)
}

fn safe_id(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric()).take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_java_news_and_patch_notes() {
        let news = serde_json::json!({ "entries": [
            { "id": "a", "title": "Java news", "text": "t", "date": "2026-09-28", "newsType": ["Java", "News page"],
              "newsPageImage": { "url": "/v2/images/a.jpg" }, "readMoreLink": "https://www.minecraft.net/article/a" },
            { "id": "b", "title": "Bedrock news", "date": "2026-09-30", "newsType": ["Bedrock"] },
            { "id": "c", "title": "Bad link", "date": "2026-09-01", "newsType": ["Java"], "readMoreLink": "https://evil.example/x" }
        ]});
        let notes = serde_json::json!({ "entries": [
            { "id": "s", "title": "Minecraft 26.4 Snapshot 3", "type": "snapshot", "version": "26.4-snapshot-3",
              "date": "2026-10-06T12:54:26.000Z", "shortText": "Brr", "contentPath": "javaPatchNotes/abc.json", "image": { "url": "/v2/images/s.jpg" } }
        ]});
        let items = parse(&news, &notes);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["s", "a", "c"]);
        assert_eq!(items[0].kind, "snapshot");
        assert_eq!(items[0].date, "2026-10-06");
        assert_eq!(items[1].image.as_deref(), Some("https://launchercontent.mojang.com/v2/images/a.jpg"));
        assert!(items[1].link.is_some() && items[2].link.is_none());
    }

    #[test]
    fn guards_paths_and_links() {
        assert!(safe_content_path("javaPatchNotes/8df071b98ac9.json"));
        assert!(!safe_content_path("javaPatchNotes/../../x.json"));
        assert!(!safe_content_path("https://evil/x.json"));
        assert!(allowed_link("https://www.minecraft.net/en-us/article/x"));
        assert!(!allowed_link("http://www.minecraft.net/x"));
        assert!(!allowed_link("https://www.minecraft.net.evil.com/x"));
    }

    /// Real feed into .minecraft/signal-cache: `cargo test --lib live_news -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_news() {
        let feed = tauri::async_runtime::block_on(feed(true)).unwrap();
        for i in feed.items.iter().take(6) {
            println!("{} [{}] {} | image {} | link {:?} | notes {:?}", i.date, i.kind, i.title, i.image.is_some(), i.link, i.content_path.is_some());
        }
        println!("items {}, with images {}", feed.items.len(), feed.items.iter().filter(|i| i.image.is_some()).count());
        let notes = feed.items.iter().find_map(|i| i.content_path.clone()).unwrap();
        let body = tauri::async_runtime::block_on(article(&notes)).unwrap();
        println!("article: {} chars, starts {:?}", body.len(), body.chars().take(80).collect::<String>());
    }
}
