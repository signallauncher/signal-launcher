//! CurseForge catalog (API v1) next to Modrinth: search, one-click install of mods (with required
//! dependencies), resource packs and shaders, and modpacks as new installations.
//!
//! The API needs a key (`x-api-key`). It belongs to the launcher's developer and is baked into
//! the build (see build.rs: `SIGNAL_CF_API_KEY` or `src-tauri/curseforge-api.key`); players never
//! enter anything. `CURSEFORGE_API_KEY` in the environment overrides it for development. Without a
//! working key the CurseForge source stays visible and shows CurseForge's refusal in the list. Keys issued to other launchers are not used: they
//! belong to those projects and CurseForge can revoke them at any time.
//!
//! Authors can forbid third-party downloads (`allowModDistribution: false`, `downloadUrl: null`);
//! such files are never fetched — the UI offers the CurseForge page instead.

use std::fs::{self, File};
use std::path::Path;

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

use crate::game::{self, Loader, Progress};
use crate::mods::{InstallReport, InstalledMod, SkippedMod};

const API: &str = "https://api.curseforge.com/v1";
const MINECRAFT: u32 = 432;
const PAGE: u32 = 20;
/// The API refuses `index + pageSize > 10000`.
const MAX_INDEX: u32 = 10_000;
/// Hosts CurseForge serves files and logos from.
const FILE_HOSTS: [&str; 3] = ["edge.forgecdn.net", "mediafilez.forgecdn.net", "media.forgecdn.net"];
const PARALLEL: usize = 6;

/// A file the author allows only on curseforge.com (sent to the UI with the error).
pub const BLOCKED: &str = "CF_BLOCKED";

// ---------------------------------------------------------------------------------------------
// API key
// ---------------------------------------------------------------------------------------------

fn env_key() -> Option<String> {
    std::env::var("CURSEFORGE_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

fn build_key() -> Option<String> {
    option_env!("SIGNAL_CF_API_KEY").map(str::trim).filter(|k| !k.is_empty()).map(str::to_string)
}

fn api_key() -> Result<String, String> {
    env_key().or_else(build_key).ok_or_else(|| "Каталог CurseForge недоступен в этой сборке лаунчера".to_string())
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

async fn get(client: &reqwest::Client, key: &str, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
    let response = client
        .get(format!("{API}{path}"))
        .header("x-api-key", key)
        .query(query)
        .send()
        .await
        .map_err(|e| format!("CurseForge недоступен: {e}"))?;
    read(response).await
}

async fn post(client: &reqwest::Client, key: &str, path: &str, body: Value) -> Result<Value, String> {
    let response = client
        .post(format!("{API}{path}"))
        .header("x-api-key", key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("CurseForge недоступен: {e}"))?;
    read(response).await
}

async fn read(response: reqwest::Response) -> Result<Value, String> {
    match response.status().as_u16() {
        200 => response.json().await.map_err(|e| format!("CurseForge: {e}")),
        401 | 403 => Err("CurseForge отклонил запрос лаунчера (403)".into()),
        404 => Err("CurseForge: проект не найден".into()),
        code => Err(format!("CurseForge: HTTP {code}")),
    }
}

fn class_id(kind: &str) -> Result<u32, String> {
    match kind {
        "mod" => Ok(6),
        "modpack" => Ok(4471),
        "resourcepack" => Ok(12),
        "shader" => Ok(6552),
        _ => Err("Неизвестный тип проекта".into()),
    }
}

fn folder_for_class(class: u64) -> &'static str {
    match class {
        12 => "resourcepacks",
        6552 => "shaderpacks",
        _ => "mods",
    }
}

/// ModLoaderType: 1 Forge, 4 Fabric, 5 Quilt, 6 NeoForge.
fn loader_type(loader: &str) -> Option<u32> {
    match loader {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

fn loader_name(t: u64) -> Option<&'static str> {
    match t {
        1 => Some("forge"),
        4 => Some("fabric"),
        5 => Some("quilt"),
        6 => Some("neoforge"),
        _ => None,
    }
}

fn allowed_host(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    FILE_HOSTS.contains(&host)
}

// ---------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CfHit {
    pub source: &'static str,
    pub project_id: String,
    pub slug: String,
    pub project_type: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    pub follows: u64,
    pub icon_url: Option<String>,
    /// Category names as CurseForge shows them, then loaders (`fabric`, `forge`…).
    pub categories: Vec<String>,
    pub date_modified: String,
    /// `false` when the author allows downloads only on curseforge.com.
    pub allow_distribution: bool,
    pub page_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CfSearchResult {
    pub hits: Vec<CfHit>,
    pub total: u64,
    pub offset: u32,
}

pub async fn search(q: crate::modrinth::SearchQuery) -> Result<CfSearchResult, String> {
    let key = api_key()?;
    let class = class_id(&q.project_type)?;
    let offset = q.offset.min(MAX_INDEX - PAGE);
    // ModsSearchSortField: 2 Popularity, 3 LastUpdated, 6 TotalDownloads, 11 ReleasedDate.
    let sort = match q.sort.as_str() {
        "downloads" => 6,
        "updated" => 3,
        "newest" => 11,
        _ => 2,
    };
    let mut query: Vec<(&str, String)> = vec![
        ("gameId", MINECRAFT.to_string()),
        ("classId", class.to_string()),
        ("sortField", sort.to_string()),
        ("sortOrder", "desc".into()),
        ("index", offset.to_string()),
        ("pageSize", PAGE.to_string()),
    ];
    if !q.query.trim().is_empty() {
        query.push(("searchFilter", q.query.trim().to_string()));
    }
    if !q.game_version.is_empty() && q.project_type != "modpack" {
        query.push(("gameVersion", q.game_version.clone()));
    }
    if matches!(q.project_type.as_str(), "mod" | "modpack") {
        if let Some(t) = loader_type(&q.loader) {
            query.push(("modLoaderType", t.to_string()));
        }
    }
    let client = game::http_client()?;
    let data = get(&client, &key, "/mods/search", &query).await?;
    let hits = data
        .get("data")
        .and_then(Value::as_array)
        .map(|list| list.iter().map(|m| hit(m, &q.project_type)).collect())
        .unwrap_or_default();
    let total = data.pointer("/pagination/totalCount").and_then(Value::as_u64).unwrap_or(0).min(MAX_INDEX as u64);
    Ok(CfSearchResult { hits, total, offset })
}

fn hit(m: &Value, kind: &str) -> CfHit {
    let text = |ptr: &str| m.pointer(ptr).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut categories: Vec<String> = m
        .get("categories")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(|x| x.get("name").and_then(Value::as_str)).take(3).map(str::to_string).collect())
        .unwrap_or_default();
    let mut loaders: Vec<String> = m
        .get("latestFilesIndexes")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|f| f.get("modLoader").and_then(Value::as_u64)).filter_map(loader_name).map(str::to_string).collect())
        .unwrap_or_default();
    loaders.sort();
    loaders.dedup();
    categories.extend(loaders);
    let icon = text("/logo/thumbnailUrl");
    CfHit {
        source: "cf",
        project_id: m.get("id").and_then(Value::as_u64).map(|n| n.to_string()).unwrap_or_default(),
        slug: text("/slug"),
        project_type: kind.to_string(),
        title: text("/name"),
        description: text("/summary"),
        author: text("/authors/0/name"),
        downloads: m.get("downloadCount").and_then(Value::as_f64).unwrap_or(0.0) as u64,
        follows: m.get("thumbsUpCount").and_then(Value::as_u64).unwrap_or(0),
        icon_url: allowed_host(&icon).then_some(icon),
        categories,
        date_modified: text("/dateModified"),
        // Missing flag = allowed; the file's own downloadUrl is checked again at install.
        allow_distribution: m.get("allowModDistribution").and_then(Value::as_bool).unwrap_or(true),
        page_url: text("/links/websiteUrl"),
    }
}

// ---------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------

struct CfFile {
    id: u64,
    name: String,
    url: Option<String>,
    sha1: Option<String>,
    size: Option<u64>,
    required_deps: Vec<u64>,
}

fn parse_file(f: &Value) -> Option<CfFile> {
    let name = f.get("fileName")?.as_str()?.to_string();
    if name.contains(['/', '\\']) || name.contains("..") {
        return None;
    }
    Some(CfFile {
        id: f.get("id")?.as_u64()?,
        name,
        url: f.get("downloadUrl").and_then(Value::as_str).filter(|u| allowed_host(u)).map(str::to_string),
        sha1: f
            .get("hashes")
            .and_then(Value::as_array)
            .and_then(|h| h.iter().find(|x| x.get("algo").and_then(Value::as_u64) == Some(1)))
            .and_then(|x| x.get("value").and_then(Value::as_str))
            .map(str::to_lowercase),
        size: f.get("fileLength").and_then(Value::as_u64),
        required_deps: f
            .get("dependencies")
            .and_then(Value::as_array)
            .map(|d| {
                d.iter()
                    .filter(|x| x.get("relationType").and_then(Value::as_u64) == Some(3))
                    .filter_map(|x| x.get("modId").and_then(Value::as_u64))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Newest fitting file of a project: releases first, then betas, then alphas.
async fn best_file(client: &reqwest::Client, key: &str, mod_id: u64, mc: Option<&str>, loader: Option<u32>) -> Result<Option<CfFile>, String> {
    let mut query: Vec<(&str, String)> = vec![("pageSize", "50".into())];
    if let Some(mc) = mc {
        query.push(("gameVersion", mc.to_string()));
    }
    if let Some(t) = loader {
        query.push(("modLoaderType", t.to_string()));
    }
    let data = get(client, key, &format!("/mods/{mod_id}/files"), &query).await?;
    let mut files: Vec<Value> = data.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    files.retain(|f| !f.get("isServerPack").and_then(Value::as_bool).unwrap_or(false));
    // Newest first, release (1) before beta (2) before alpha (3).
    files.sort_by(|a, b| {
        let rt = |f: &Value| f.get("releaseType").and_then(Value::as_u64).unwrap_or(3);
        let date = |f: &Value| f.get("fileDate").and_then(Value::as_str).unwrap_or_default().to_string();
        rt(a).cmp(&rt(b)).then_with(|| date(b).cmp(&date(a)))
    });
    Ok(files.first().and_then(parse_file))
}

async fn fetch_file(client: &reqwest::Client, file: &CfFile, dest: &Path) -> Result<(), String> {
    let url = file.url.as_deref().ok_or(BLOCKED)?;
    match &file.sha1 {
        Some(sha1) => crate::mrpack::download_verified(client, url, dest, sha1, file.size).await,
        None => game::download(client, url, dest, file.size).await,
    }
}

/// Mod (with required dependencies), resource pack or shader into the installation.
/// An author-blocked main file fails with `CF_BLOCKED: …` so the UI can offer the website.
pub async fn install(instance_id: &str, instance_name: &str, version_label: &str, mod_id: &str, kind: &str) -> Result<InstallReport, String> {
    let key = api_key()?;
    let mod_id: u64 = mod_id.parse().map_err(|_| "Некорректный id проекта".to_string())?;
    let (loader, mc) = game::parse_label(version_label);
    let mc = mc.ok_or_else(|| format!("Не удалось определить версию игры из «{version_label}»"))?;
    let loader_t = match (kind, loader) {
        ("mod", Loader::Fabric) => Some(4),
        ("mod", Loader::Forge) => Some(1),
        ("mod", Loader::NeoForge) => Some(6),
        ("mod", Loader::Vanilla) => return Err("Моды ставятся в установку с Fabric, Forge или NeoForge".into()),
        _ => None,
    };
    let folder = match kind {
        "mod" => "mods",
        "resourcepack" => "resourcepacks",
        "shader" => "shaderpacks",
        _ => return Err("Неизвестный тип проекта".into()),
    };
    let inst = game::instance_dir(instance_id);
    if !inst.join("instance.json").is_file() {
        game::write_instance_meta(instance_id, instance_name, version_label);
    }
    let dir = inst.join(folder);
    let client = game::http_client()?;

    let mut report = InstallReport { installed: Vec::new(), skipped: Vec::new(), mods_dir: game::path_str(&dir) };
    let mut queue = vec![(mod_id, false)];
    let mut seen = std::collections::HashSet::new();
    while let Some((id, dependency)) = queue.pop() {
        if !seen.insert(id) || seen.len() > 40 {
            continue;
        }
        // Resource packs often list no game version: fall back to the newest file.
        let mut file = best_file(&client, &key, id, Some(&mc), loader_t).await?;
        if file.is_none() && kind != "mod" {
            file = best_file(&client, &key, id, None, None).await?;
        }
        let Some(file) = file else {
            if dependency {
                report.skipped.push(SkippedMod { name: format!("CurseForge #{id}"), reason: format!("нет версии под Minecraft {mc}") });
                continue;
            }
            return Err(format!("нет версии под Minecraft {mc}"));
        };
        let dest = dir.join(&file.name);
        if dependency && dest.is_file() {
            continue;
        }
        match fetch_file(&client, &file, &dest).await {
            Ok(()) => {}
            Err(e) if e == BLOCKED && !dependency => {
                return Err(format!("{BLOCKED}: автор разрешил скачивать «{}» только с сайта CurseForge", file.name));
            }
            Err(e) => {
                let reason = if e == BLOCKED { "автор разрешил скачивание только с сайта CurseForge".to_string() } else { e };
                report.skipped.push(SkippedMod { name: file.name.clone(), reason });
                continue;
            }
        }
        report.installed.push(InstalledMod { name: file.name.clone(), project_id: id.to_string(), file: file.name.clone(), dependency });
        if kind == "mod" {
            queue.extend(file.required_deps.iter().map(|d| (*d, true)));
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------------------------
// Modpacks (manifest.json + overrides/)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    minecraft: ManifestGame,
    #[serde(default)]
    files: Vec<ManifestFile>,
    #[serde(default)]
    overrides: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestGame {
    version: String,
    #[serde(default)]
    mod_loaders: Vec<ManifestLoader>,
}

#[derive(Debug, Deserialize)]
struct ManifestLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    #[serde(default = "yes")]
    required: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedFile {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CfImportReport {
    pub instance_id: String,
    pub name: String,
    pub version: String,
    pub loader_version: Option<String>,
    pub files: usize,
    pub server_only: usize,
    pub overrides: usize,
    pub icon: Option<String>,
    /// Files the authors allow only on curseforge.com: download them by hand into the folder.
    pub blocked: Vec<BlockedFile>,
}

/// `forge-47.2.0` → (`Forge 1.20.1`, `47.2.0`)
fn manifest_label(game: &ManifestGame) -> Result<(String, Option<String>), String> {
    let mc = game.version.trim();
    let primary = game.mod_loaders.iter().find(|l| l.primary).or_else(|| game.mod_loaders.first());
    let Some(loader) = primary else {
        return Ok((format!("Release {mc}"), None));
    };
    let id = loader.id.trim();
    let (name, version) = id.split_once('-').ok_or_else(|| format!("Непонятный загрузчик сборки: {id}"))?;
    let label = match name {
        "fabric" => "Fabric",
        "forge" => "Forge",
        "neoforge" => "NeoForge",
        "quilt" => return Err("Сборки на Quilt пока не поддерживаются: Signal умеет Fabric, Forge и NeoForge.".into()),
        other => return Err(format!("Загрузчик «{other}» не поддерживается")),
    };
    // NeoForge 1.20.1 versions look like `neoforge-1.20.1-47.1.x`.
    let version = version.strip_prefix(&format!("{mc}-")).unwrap_or(version);
    Ok((format!("{label} {mc}"), Some(version.to_string())))
}

/// A CurseForge modpack from the catalog → new installation. Progress: `import-progress`.
pub async fn install_modpack(app: AppHandle, mod_id: &str) -> Result<CfImportReport, String> {
    let key = api_key()?;
    let mod_id: u64 = mod_id.parse().map_err(|_| "Некорректный id проекта".to_string())?;
    let progress = Progress::new(app, "import-progress");
    let client = game::http_client()?;
    progress.step("read", "Скачивание сборки с CurseForge…", 1.0);
    let project = get(&client, &key, &format!("/mods/{mod_id}"), &[]).await?;
    let project = project.get("data").cloned().unwrap_or_default();
    let file = best_file(&client, &key, mod_id, None, None).await?.ok_or("У сборки нет опубликованных файлов")?;
    let tmp = game::minecraft_dir().join("signal-cache").join(format!("cf-{}-{}", mod_id, file.name));
    match fetch_file(&client, &file, &tmp).await {
        Err(e) if e == BLOCKED => return Err(format!("{BLOCKED}: автор разрешил скачивать сборку только с сайта CurseForge")),
        other => other?,
    }
    let id = format!("m{}", game::now_ms());
    let dir = game::instance_dir(&id);
    let result = unpack(&client, &key, &progress, &tmp, &dir).await;
    let _ = fs::remove_file(&tmp);
    match result {
        Ok((manifest, label, loader_version, counts, blocked)) => {
            let title = if manifest.name.trim().is_empty() { project.get("name").and_then(Value::as_str).unwrap_or("Сборка").to_string() } else { manifest.name.trim().to_string() };
            let icon = match project.pointer("/logo/thumbnailUrl").and_then(Value::as_str).filter(|u| allowed_host(u)) {
                Some(url) => {
                    let dest = dir.join("icon.png");
                    game::download(&client, url, &dest, None).await.ok().map(|_| game::path_str(&dest))
                }
                None => None,
            };
            game::update_instance_meta(&id, |meta| {
                meta.insert("id".into(), id.clone().into());
                meta.insert("name".into(), title.clone().into());
                meta.insert("version".into(), label.clone().into());
                if let Some(lv) = &loader_version {
                    meta.insert("loaderVersion".into(), lv.clone().into());
                }
                meta.insert("pack".into(), serde_json::json!({ "name": title, "version": manifest.version, "source": "curseforge", "projectId": mod_id }));
            });
            progress.step("done", "Сборка установлена", 100.0);
            Ok(CfImportReport { instance_id: id, name: title, version: label, loader_version, files: counts.0, server_only: 0, overrides: counts.1, icon, blocked })
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

type Unpacked = (Manifest, String, Option<String>, (usize, usize), Vec<BlockedFile>);

async fn unpack(client: &reqwest::Client, key: &str, progress: &Progress, pack: &Path, dir: &Path) -> Result<Unpacked, String> {
    progress.step("read", "Чтение сборки…", 4.0);
    let manifest: Manifest = {
        let mut archive = open_zip(pack)?;
        let mut entry = archive.by_name("manifest.json").map_err(|_| "В архиве нет manifest.json — это не сборка CurseForge".to_string())?;
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("manifest.json: {e}"))?
    };
    let (label, loader_version) = manifest_label(&manifest.minecraft)?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let wanted: Vec<&ManifestFile> = manifest.files.iter().filter(|f| f.required).collect();
    let file_ids: Vec<u64> = wanted.iter().map(|f| f.file_id).collect();
    let mod_ids: Vec<u64> = wanted.iter().map(|f| f.project_id).collect();
    let mut files: Vec<(CfFile, u64)> = Vec::new();
    let mut classes = std::collections::HashMap::new();
    let mut pages = std::collections::HashMap::new();
    for chunk in mod_ids.chunks(200) {
        let data = post(client, key, "/mods", serde_json::json!({ "modIds": chunk })).await?;
        for m in data.get("data").and_then(Value::as_array).cloned().unwrap_or_default() {
            if let Some(id) = m.get("id").and_then(Value::as_u64) {
                classes.insert(id, m.get("classId").and_then(Value::as_u64).unwrap_or(6));
                pages.insert(id, m.pointer("/links/websiteUrl").and_then(Value::as_str).unwrap_or_default().to_string());
            }
        }
    }
    for chunk in file_ids.chunks(200) {
        let data = post(client, key, "/mods/files", serde_json::json!({ "fileIds": chunk })).await?;
        for f in data.get("data").and_then(Value::as_array).cloned().unwrap_or_default() {
            let project = f.get("modId").and_then(Value::as_u64).unwrap_or(0);
            if let Some(file) = parse_file(&f) {
                files.push((file, project));
            }
        }
    }
    let mut blocked = Vec::new();
    let mut jobs = Vec::new();
    for (file, project) in files {
        let folder = folder_for_class(classes.get(&project).copied().unwrap_or(6));
        if file.url.is_none() {
            let page = pages.get(&project).cloned().unwrap_or_default();
            blocked.push(BlockedFile { url: if page.is_empty() { String::new() } else { format!("{page}/files/{}", file.id) }, name: format!("{folder}/{}", file.name) });
            continue;
        }
        jobs.push((dir.join(folder).join(&file.name), file));
    }

    let total = jobs.len();
    progress.report("files", format!("Скачивание файлов сборки (0/{total})…"), 6.0, 0, total, true);
    let mut results = Box::pin(
        stream::iter(jobs.into_iter().map(|(dest, file)| async move { fetch_file(client, &file, &dest).await.map_err(|e| format!("{}: {e}", file.name)) }))
            .buffer_unordered(PARALLEL),
    );
    let mut done = 0usize;
    while let Some(result) = results.next().await {
        result?;
        done += 1;
        progress.report("files", format!("Скачивание файлов сборки ({done}/{total})…"), 6.0 + 84.0 * done as f64 / total.max(1) as f64, done, total, done == total);
    }

    progress.step("overrides", "Копирование настроек сборки (overrides)…", 92.0);
    let prefix = format!("{}/", manifest.overrides.as_deref().unwrap_or("overrides").trim_matches('/'));
    let (pack, target) = (pack.to_path_buf(), dir.to_path_buf());
    let overrides = tauri::async_runtime::spawn_blocking(move || copy_overrides(&pack, &prefix, &target)).await.map_err(|e| e.to_string())??;
    Ok((manifest, label, loader_version, (total, overrides), blocked))
}

fn open_zip(pack: &Path) -> Result<zip::ZipArchive<File>, String> {
    let file = File::open(pack).map_err(|e| format!("{}: {e}", pack.display()))?;
    zip::ZipArchive::new(file).map_err(|e| format!("Повреждённый архив сборки: {e}"))
}

fn copy_overrides(pack: &Path, prefix: &str, dir: &Path) -> Result<usize, String> {
    let mut archive = open_zip(pack)?;
    let mut count = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().replace('\\', "/");
        let Some(rel) = name.strip_prefix(prefix).filter(|r| !r.is_empty()) else { continue };
        let rel = crate::mrpack::safe_relative(rel).ok_or_else(|| format!("Недопустимый путь в overrides: {name}"))?;
        let dest = dir.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut out = File::create(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", dest.display()))?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_hit() {
        let m = serde_json::json!({
            "id": 238222, "slug": "jei", "name": "Just Enough Items (JEI)", "summary": "View Items and Recipes",
            "downloadCount": 3.21e8, "thumbsUpCount": 120, "classId": 6, "allowModDistribution": true,
            "logo": { "thumbnailUrl": "https://media.forgecdn.net/avatars/thumbnails/29/69/256/256/635838945588716414.jpeg" },
            "authors": [{ "name": "mezz" }], "categories": [{ "name": "API and Library" }],
            "links": { "websiteUrl": "https://www.curseforge.com/minecraft/mc-mods/jei" },
            "latestFilesIndexes": [{ "modLoader": 1 }, { "modLoader": 6 }, { "modLoader": 1 }],
            "dateModified": "2026-10-01T10:00:00Z"
        });
        let h = hit(&m, "mod");
        assert_eq!(h.project_id, "238222");
        assert_eq!(h.downloads, 321_000_000);
        assert_eq!(h.categories, vec!["API and Library", "forge", "neoforge"]);
        assert!(h.allow_distribution && h.icon_url.is_some());
        let blocked = hit(&serde_json::json!({ "id": 1, "allowModDistribution": false, "logo": { "thumbnailUrl": "https://evil.example/x.png" } }), "mod");
        assert!(!blocked.allow_distribution && blocked.icon_url.is_none());
    }

    #[test]
    fn parses_files_and_blocks_foreign_hosts() {
        let f = serde_json::json!({ "id": 5, "fileName": "jei-1.21.1.jar", "downloadUrl": "https://edge.forgecdn.net/files/5/5/jei.jar",
            "hashes": [{ "value": "ABC", "algo": 2 }, { "value": "DEF", "algo": 1 }], "fileLength": 10,
            "dependencies": [{ "modId": 306612, "relationType": 3 }, { "modId": 7, "relationType": 2 }] });
        let file = parse_file(&f).unwrap();
        assert_eq!(file.sha1.as_deref(), Some("def"));
        assert_eq!(file.required_deps, vec![306612]);
        assert!(file.url.is_some());
        let no_url = parse_file(&serde_json::json!({ "id": 6, "fileName": "x.jar", "downloadUrl": null })).unwrap();
        assert!(no_url.url.is_none());
        let foreign = parse_file(&serde_json::json!({ "id": 7, "fileName": "x.jar", "downloadUrl": "https://evil.example/x.jar" })).unwrap();
        assert!(foreign.url.is_none());
        assert!(parse_file(&serde_json::json!({ "id": 8, "fileName": "../x.jar" })).is_none());
    }

    #[test]
    fn manifest_loaders() {
        let game = |v: &str, id: &str| ManifestGame { version: v.into(), mod_loaders: vec![ManifestLoader { id: id.into(), primary: true }] };
        assert_eq!(manifest_label(&game("1.20.1", "forge-47.2.0")).unwrap(), ("Forge 1.20.1".to_string(), Some("47.2.0".to_string())));
        assert_eq!(manifest_label(&game("1.21.1", "fabric-0.16.9")).unwrap(), ("Fabric 1.21.1".to_string(), Some("0.16.9".to_string())));
        assert_eq!(manifest_label(&game("1.21.1", "neoforge-21.1.77")).unwrap(), ("NeoForge 1.21.1".to_string(), Some("21.1.77".to_string())));
        assert!(manifest_label(&game("1.20.1", "quilt-0.20")).is_err());
        let manifest: Manifest = serde_json::from_str(r#"{"minecraft":{"version":"1.20.1","modLoaders":[{"id":"forge-47.2.0","primary":true}]},
            "manifestType":"minecraftModpack","name":"Pack","version":"1.0","files":[{"projectID":1,"fileID":2,"required":true},{"projectID":3,"fileID":4}],"overrides":"overrides"}"#).unwrap();
        assert_eq!(manifest.files.len(), 2);
        assert!(manifest.files[1].required);
    }

    /// With the build key (curseforge-api.key) or `$env:CURSEFORGE_API_KEY`: `cargo test --lib live_cf_search -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_cf_search() {
        let key = api_key().expect("no CurseForge key: src-tauri/curseforge-api.key or CURSEFORGE_API_KEY");
        let client = game::http_client().unwrap();
        let data = tauri::async_runtime::block_on(get(&client, &key, "/mods/search", &[("gameId", "432".into()), ("classId", "6".into()), ("sortField", "6".into()), ("sortOrder", "desc".into()), ("pageSize", "5".into()), ("gameVersion", "1.21.1".into()), ("modLoaderType", "4".into())])).unwrap();
        let hits: Vec<CfHit> = data["data"].as_array().unwrap().iter().map(|m| hit(m, "mod")).collect();
        for h in &hits {
            println!("{} · {} · {} · distribution {}", h.title, h.downloads, h.categories.join(","), h.allow_distribution);
        }
        assert!(!hits.is_empty());
        let file = tauri::async_runtime::block_on(best_file(&client, &key, hits[0].project_id.parse().unwrap(), Some("1.21.1"), Some(4))).unwrap().unwrap();
        println!("best file: {} sha1 {:?} url {:?}", file.name, file.sha1, file.url);
    }
}
