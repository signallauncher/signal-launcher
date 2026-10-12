//! Live Modrinth catalog: search (mods, modpacks, resource packs, shaders) and one-click install of
//! the file that fits the installation. Mods go through `mods::install` (dependencies included);
//! resource packs and shaders are single files; a modpack becomes a new installation via
//! `mrpack::import`.

use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

use crate::game;

const API: &str = "https://api.modrinth.com/v2";
const PAGE: u32 = 20;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub query: String,
    /// `mod`, `modpack`, `resourcepack`, `shader`
    pub project_type: String,
    /// `fabric`, `forge`, `neoforge` or empty for any.
    #[serde(default)]
    pub loader: String,
    /// `1.21.1` or empty for any.
    #[serde(default)]
    pub game_version: String,
    /// `relevance`, `downloads`, `follows`, `newest`, `updated`
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub offset: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub project_id: String,
    pub slug: String,
    pub project_type: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    pub follows: u64,
    pub icon_url: Option<String>,
    pub categories: Vec<String>,
    pub date_modified: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub hits: Vec<Hit>,
    pub total: u64,
    pub offset: u32,
}

pub async fn search(q: SearchQuery) -> Result<SearchResult, String> {
    let kind = match q.project_type.as_str() {
        "mod" | "modpack" | "resourcepack" | "shader" => q.project_type.as_str(),
        _ => return Err("Неизвестный тип проекта".into()),
    };
    let mut facets: Vec<Vec<String>> = vec![vec![format!("project_type:{kind}")]];
    // Loaders only mean something for mods and modpacks.
    if matches!(kind, "mod" | "modpack") && !q.loader.is_empty() {
        facets.push(vec![format!("categories:{}", q.loader)]);
    }
    if !q.game_version.is_empty() && kind != "modpack" {
        facets.push(vec![format!("versions:{}", q.game_version)]);
    }
    let index = match q.sort.as_str() {
        "downloads" | "follows" | "newest" | "updated" => q.sort.as_str(),
        _ => "relevance",
    };
    let client = game::http_client()?;
    let response = client
        .get(format!("{API}/search"))
        .query(&[
            ("query", q.query.trim().to_string()),
            ("facets", serde_json::to_string(&facets).unwrap_or_default()),
            ("index", index.to_string()),
            ("offset", q.offset.to_string()),
            ("limit", PAGE.to_string()),
        ])
        .send()
        .await
        .map_err(|e| format!("Modrinth недоступен: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Modrinth: HTTP {}", response.status().as_u16()));
    }
    let data: Value = response.json().await.map_err(|e| format!("Modrinth: {e}"))?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let hits = data
        .get("hits")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|h| Hit {
                    project_id: text(h, "project_id"),
                    slug: text(h, "slug"),
                    project_type: text(h, "project_type"),
                    title: text(h, "title"),
                    description: text(h, "description"),
                    author: text(h, "author"),
                    downloads: h.get("downloads").and_then(Value::as_u64).unwrap_or(0),
                    follows: h.get("follows").and_then(Value::as_u64).unwrap_or(0),
                    icon_url: h.get("icon_url").and_then(Value::as_str).filter(|u| u.starts_with("https://cdn.modrinth.com/")).map(str::to_string),
                    categories: h
                        .get("display_categories")
                        .and_then(Value::as_array)
                        .map(|c| c.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default(),
                    date_modified: text(h, "date_modified"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SearchResult { hits, total: data.get("total_hits").and_then(Value::as_u64).unwrap_or(0), offset: q.offset })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInstall {
    pub file: String,
    pub version: String,
}

/// Resource pack or shader into `resourcepacks/` / `shaderpacks/` of the installation: the newest
/// release for its game version (shaders: Iris builds preferred), sha1-checked.
pub async fn install_asset(instance_id: &str, version_label: &str, project_id: &str, kind: &str) -> Result<AssetInstall, String> {
    let folder = match kind {
        "resourcepack" => "resourcepacks",
        "shader" => "shaderpacks",
        _ => return Err("Неизвестный тип проекта".into()),
    };
    let (_, mc) = game::parse_label(version_label);
    let mc = mc.ok_or("Не удалось определить версию игры установки")?;
    let client = game::http_client()?;
    let versions = project_versions(&client, project_id, Some(&mc)).await?;
    let preferred = |v: &&Value| kind != "shader" || loaders(v).iter().any(|l| l == "iris");
    let pick = versions
        .iter()
        .filter(preferred)
        .find(|v| v.get("version_type").and_then(Value::as_str) == Some("release"))
        .or_else(|| versions.iter().find(|v| v.get("version_type").and_then(Value::as_str) == Some("release")))
        .or_else(|| versions.first())
        .ok_or_else(|| format!("нет версии под Minecraft {mc}"))?;
    let (url, filename, sha1, size) = primary_file(pick).ok_or("у версии нет файла")?;
    let dest = game::instance_dir(instance_id).join(folder).join(&filename);
    crate::mrpack::download_verified(&client, &url, &dest, &sha1, size).await?;
    Ok(AssetInstall { file: filename, version: pick.get("version_number").and_then(Value::as_str).unwrap_or_default().to_string() })
}

/// A modpack from the catalog: its newest release `.mrpack` is downloaded and imported as a new
/// installation (progress: `import-progress`).
pub async fn install_modpack(app: AppHandle, project_id: &str) -> Result<crate::mrpack::ImportReport, String> {
    let client = game::http_client()?;
    let versions = project_versions(&client, project_id, None).await?;
    let pick = versions
        .iter()
        .find(|v| v.get("version_type").and_then(Value::as_str) == Some("release"))
        .or_else(|| versions.first())
        .ok_or("У сборки нет опубликованных версий")?;
    let (url, filename, sha1, size) = primary_file(pick).ok_or("у версии нет файла")?;
    if !filename.to_lowercase().ends_with(".mrpack") {
        return Err("У этой сборки нет файла .mrpack".into());
    }
    let tmp = game::minecraft_dir().join("signal-cache").join(&filename);
    crate::mrpack::download_verified(&client, &url, &tmp, &sha1, size).await?;
    let result = crate::mrpack::import(app, game::path_str(&tmp)).await;
    let _ = fs::remove_file(&tmp);
    result
}

async fn project_versions(client: &reqwest::Client, project_id: &str, mc: Option<&str>) -> Result<Vec<Value>, String> {
    if project_id.is_empty() || !project_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Некорректный id проекта".into());
    }
    let mut request = client.get(format!("{API}/project/{project_id}/version"));
    if let Some(mc) = mc {
        request = request.query(&[("game_versions", format!("[\"{mc}\"]"))]);
    }
    let response = request.send().await.map_err(|e| format!("Modrinth недоступен: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Modrinth: HTTP {}", response.status().as_u16()));
    }
    response.json().await.map_err(|e| format!("Modrinth: {e}"))
}

fn loaders(version: &Value) -> Vec<String> {
    version
        .get("loaders")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).map(str::to_lowercase).collect())
        .unwrap_or_default()
}

/// (url, file name, sha1, size) of the primary file; the name must be a plain file name.
fn primary_file(version: &Value) -> Option<(String, String, String, Option<u64>)> {
    let files = version.get("files")?.as_array()?;
    let f = files
        .iter()
        .find(|f| f.get("primary").and_then(Value::as_bool) == Some(true))
        .or_else(|| files.first())?;
    let name = f.get("filename")?.as_str()?;
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }
    Some((
        f.get("url")?.as_str()?.to_string(),
        name.to_string(),
        f.pointer("/hashes/sha1")?.as_str()?.to_string(),
        f.get("size").and_then(Value::as_u64),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test --lib live_search -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_search() {
        for (kind, loader, mc) in [("mod", "fabric", "1.21.1"), ("modpack", "fabric", ""), ("resourcepack", "", "1.21.1"), ("shader", "", "1.21.1")] {
            let q = SearchQuery { query: String::new(), project_type: kind.into(), loader: loader.into(), game_version: mc.into(), sort: "downloads".into(), offset: 0 };
            let r = tauri::async_runtime::block_on(search(q)).unwrap();
            println!("{kind}: total {}, first: {}", r.total, r.hits.iter().take(3).map(|h| h.title.as_str()).collect::<Vec<_>>().join(", "));
            assert!(!r.hits.is_empty());
        }
    }
}
