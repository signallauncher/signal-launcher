//! Mods from Modrinth: finds the right `.jar` for the installation's game version and loader,
//! pulls required dependencies and saves everything into `.minecraft/instances/<id>/mods`.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

use crate::game::{self, Loader, Progress};

const MODRINTH_API: &str = "https://api.modrinth.com/v2";

/// One mod the UI asks for: a Modrinth slug/id when known (catalog), otherwise just a name (AI builder).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModRequest {
    pub name: String,
    pub project_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledMod {
    pub name: String,
    pub project_id: String,
    pub file: String,
    pub dependency: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedMod {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub installed: Vec<InstalledMod>,
    pub skipped: Vec<SkippedMod>,
    pub mods_dir: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModFile {
    pub file: String,
    pub size: u64,
    pub enabled: bool,
}

struct Pending {
    name: String,
    project: Option<String>,
    dependency: bool,
}

pub async fn install(
    app: AppHandle,
    instance_id: String,
    instance_name: String,
    version_label: String,
    requests: Vec<ModRequest>,
) -> Result<InstallReport, String> {
    let (loader, game_version) = game::parse_label(&version_label);
    let game_version = game_version.ok_or_else(|| format!("Не удалось определить версию игры из «{version_label}»"))?;
    let loader = match loader {
        Loader::Fabric => "fabric",
        Loader::Forge => "forge",
        Loader::NeoForge => "neoforge",
        Loader::Vanilla => {
            return Err("В ванильную установку моды не ставятся — выберите установку с Fabric, Forge или NeoForge.".into())
        }
    };

    let mods_dir = game::instance_dir(&instance_id).join("mods");
    fs::create_dir_all(&mods_dir).map_err(|e| format!("{}: {e}", mods_dir.display()))?;
    game::write_instance_meta(&instance_id, &instance_name, &version_label);

    let client = game::http_client()?;
    let progress = Progress::new(app, "mods-progress");
    let mut queue: VecDeque<Pending> = requests
        .into_iter()
        .map(|r| Pending { name: r.name, project: r.project_id, dependency: false })
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut report = InstallReport {
        installed: Vec::new(),
        skipped: Vec::new(),
        mods_dir: game::path_str(&mods_dir),
    };
    let mut done = 0usize;

    while let Some(item) = queue.pop_front() {
        let total = done + queue.len() + 1;
        progress.report(
            "mods",
            format!("Поиск «{}» на Modrinth ({}/{total})…", item.name, done + 1),
            100.0 * done as f64 / total as f64,
            done,
            total,
            true,
        );

        let project = match &item.project {
            Some(id) => Some(id.clone()),
            None => search_project(&client, &item.name, loader, &game_version).await?,
        };
        let Some(project) = project else {
            report.skipped.push(SkippedMod { name: item.name, reason: "не найден на Modrinth".into() });
            done += 1;
            continue;
        };
        if !seen.insert(project.clone()) {
            done += 1;
            continue;
        }

        let version = match pick_version(&client, &project, loader, &game_version).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                report.skipped.push(SkippedMod {
                    name: item.name,
                    reason: format!("нет версии под {loader} {game_version}"),
                });
                done += 1;
                continue;
            }
            Err(e) => {
                report.skipped.push(SkippedMod { name: item.name, reason: e });
                done += 1;
                continue;
            }
        };
        let Some(file) = primary_file(&version) else {
            report.skipped.push(SkippedMod { name: item.name, reason: "у версии нет файла".into() });
            done += 1;
            continue;
        };

        progress.report(
            "mods",
            format!("Скачивание {} ({}/{total})…", file.filename, done + 1),
            100.0 * (done as f64 + 0.5) / total as f64,
            done,
            total,
            true,
        );
        game::download(&client, &file.url, &mods_dir.join(&file.filename), file.size).await?;
        report.installed.push(InstalledMod {
            name: item.name,
            project_id: project.clone(),
            file: file.filename,
            dependency: item.dependency,
        });

        // Required dependencies (e.g. Fabric API) go to the back of the queue.
        if let Some(deps) = version.get("dependencies").and_then(Value::as_array) {
            for dep in deps {
                if dep.get("dependency_type").and_then(Value::as_str) != Some("required") {
                    continue;
                }
                if let Some(dep_project) = dep.get("project_id").and_then(Value::as_str) {
                    if !seen.contains(dep_project) {
                        queue.push_back(Pending {
                            name: dep_project.to_string(),
                            project: Some(dep_project.to_string()),
                            dependency: true,
                        });
                    }
                }
            }
        }
        done += 1;
    }

    progress.report("mods", "Готово", 100.0, done, done, true);
    Ok(report)
}

/// Best Modrinth project for a free-text name, filtered by loader and game version.
async fn search_project(
    client: &reqwest::Client,
    name: &str,
    loader: &str,
    game_version: &str,
) -> Result<Option<String>, String> {
    let query = clean_query(name);
    let facets = format!(r#"[["project_type:mod"],["categories:{loader}"],["versions:{game_version}"]]"#);
    let response = client
        .get(format!("{MODRINTH_API}/search"))
        .query(&[("query", query.as_str()), ("facets", facets.as_str()), ("limit", "5")])
        .send()
        .await
        .map_err(|e| format!("Modrinth недоступен: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Modrinth: HTTP {}", response.status().as_u16()));
    }
    let data: Value = response.json().await.map_err(|e| format!("Modrinth: {e}"))?;
    let hits = data.get("hits").and_then(Value::as_array).cloned().unwrap_or_default();
    let wanted = query.to_lowercase();
    let exact = hits.iter().find(|h| {
        h.get("title").and_then(Value::as_str).map(|t| clean_query(t).to_lowercase()) == Some(wanted.clone())
            || h.get("slug").and_then(Value::as_str).map(str::to_lowercase) == Some(wanted.replace(' ', "-"))
    });
    Ok(exact
        .or_else(|| hits.first())
        .and_then(|h| h.get("project_id").and_then(Value::as_str))
        .map(str::to_string))
}

/// "Just Enough Items (JEI)" → "Just Enough Items", "Jade 🔍" → "Jade".
fn clean_query(name: &str) -> String {
    let base = name.split('(').next().unwrap_or(name);
    base.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || matches!(c, '\'' | '-' | '+' | '&'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Newest release (or newest of any type) of a project for the loader + game version.
async fn pick_version(
    client: &reqwest::Client,
    project: &str,
    loader: &str,
    game_version: &str,
) -> Result<Option<Value>, String> {
    let loaders = format!(r#"["{loader}"]"#);
    let versions = format!(r#"["{game_version}"]"#);
    let response = client
        .get(format!("{MODRINTH_API}/project/{project}/version"))
        .query(&[("loaders", loaders.as_str()), ("game_versions", versions.as_str())])
        .send()
        .await
        .map_err(|e| format!("Modrinth недоступен: {e}"))?;
    if response.status().as_u16() == 404 {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("Modrinth: HTTP {}", response.status().as_u16()));
    }
    let list: Vec<Value> = response.json().await.map_err(|e| format!("Modrinth: {e}"))?;
    let release = list
        .iter()
        .find(|v| v.get("version_type").and_then(Value::as_str) == Some("release"))
        .cloned();
    Ok(release.or_else(|| list.into_iter().next()))
}

struct RemoteFile {
    url: String,
    filename: String,
    size: Option<u64>,
}

fn primary_file(version: &Value) -> Option<RemoteFile> {
    let files = version.get("files")?.as_array()?;
    let file = files
        .iter()
        .find(|f| f.get("primary").and_then(Value::as_bool) == Some(true))
        .or_else(|| files.first())?;
    let filename = file.get("filename")?.as_str()?;
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return None;
    }
    Some(RemoteFile {
        url: file.get("url")?.as_str()?.to_string(),
        filename: filename.to_string(),
        size: file.get("size").and_then(Value::as_u64),
    })
}

/// `.jar` and `.jar.disabled` files in the installation's mods folder.
pub fn list(instance_id: &str) -> Vec<ModFile> {
    let dir = game::instance_dir(instance_id).join("mods");
    let mut files: Vec<ModFile> = fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let file = e.file_name().into_string().ok()?;
                    let enabled = file.ends_with(".jar");
                    if !enabled && !file.ends_with(".jar.disabled") {
                        return None;
                    }
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    Some(ModFile { file, size, enabled })
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|f| f.file.to_lowercase());
    files
}

fn check_name(file: &str) -> Result<(), String> {
    if file.is_empty() || file.contains('/') || file.contains('\\') || file.contains("..") {
        return Err("Некорректное имя файла мода".into());
    }
    Ok(())
}

pub fn remove(instance_id: &str, file: &str) -> Result<(), String> {
    check_name(file)?;
    let path = game::instance_dir(instance_id).join("mods").join(file);
    remove_file(&path)
}

/// Turns a mod on/off by renaming `name.jar` ↔ `name.jar.disabled`; returns the new file name.
pub fn set_enabled(instance_id: &str, file: &str, enabled: bool) -> Result<String, String> {
    check_name(file)?;
    let dir = game::instance_dir(instance_id).join("mods");
    let path = dir.join(file);
    if !path.is_file() {
        return Err(format!("Файл {file} не найден в папке mods"));
    }
    let renamed = toggled_name(file, enabled);
    if renamed != file {
        let target = dir.join(&renamed);
        if target.exists() {
            return Err(format!("Уже есть файл {renamed}"));
        }
        fs::rename(&path, &target).map_err(|e| format!("Не удалось переименовать {file}: {e}"))?;
    }
    Ok(renamed)
}

fn toggled_name(file: &str, enabled: bool) -> String {
    let base = file.strip_suffix(".disabled").unwrap_or(file);
    if enabled {
        base.to_string()
    } else {
        format!("{base}.disabled")
    }
}

fn remove_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    fs::remove_file(path).map_err(|e| format!("Не удалось удалить {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------------------------
// Updates: installed files are matched to Modrinth by sha1
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdate {
    /// Installed file (`.jar` or `.jar.disabled`).
    pub file: String,
    pub project_id: String,
    pub title: String,
    pub current: String,
    pub latest: String,
    /// Modrinth version id to install.
    pub version_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReport {
    pub updates: Vec<ModUpdate>,
    /// Files Modrinth recognised.
    pub known: usize,
    /// Files it did not (CurseForge-only or self-built mods).
    pub unknown: usize,
}

/// «Проверить обновления»: hashes every mod, asks Modrinth which version each file is and what
/// the newest version for this installation's game version and loader is.
pub async fn check_updates(instance_id: &str) -> Result<UpdateReport, String> {
    let (loader, game_version) = instance_target(instance_id)?;
    let dir = game::instance_dir(instance_id).join("mods");
    let files = list(instance_id);
    let mut by_hash: Vec<(String, String)> = Vec::new();
    for f in &files {
        let bytes = fs::read(dir.join(&f.file)).map_err(|e| format!("{}: {e}", f.file))?;
        by_hash.push((crate::mrpack::sha1_hex(&bytes), f.file.clone()));
    }
    if by_hash.is_empty() {
        return Ok(UpdateReport { updates: Vec::new(), known: 0, unknown: 0 });
    }
    let hashes: Vec<&str> = by_hash.iter().map(|(h, _)| h.as_str()).collect();
    let client = game::http_client()?;
    let current: serde_json::Map<String, Value> =
        modrinth_post(&client, "/version_files", serde_json::json!({ "hashes": hashes, "algorithm": "sha1" })).await?;
    let latest: serde_json::Map<String, Value> = modrinth_post(
        &client,
        "/version_files/update",
        serde_json::json!({ "hashes": hashes, "algorithm": "sha1", "loaders": [loader], "game_versions": [game_version] }),
    )
    .await?;

    let mut updates = Vec::new();
    for (hash, file) in &by_hash {
        let (Some(now), Some(new)) = (current.get(hash), latest.get(hash)) else { continue };
        let id = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        if id(now, "id") == id(new, "id") || id(new, "id").is_empty() {
            continue;
        }
        // Never offer an older build as an "update".
        if id(new, "date_published") <= id(now, "date_published") {
            continue;
        }
        updates.push(ModUpdate {
            file: file.clone(),
            project_id: id(new, "project_id"),
            title: String::new(),
            current: id(now, "version_number"),
            latest: id(new, "version_number"),
            version_id: id(new, "id"),
        });
    }
    // Project titles for the list, in one request.
    if !updates.is_empty() {
        let ids: Vec<&str> = updates.iter().map(|u| u.project_id.as_str()).collect();
        let ids = serde_json::to_string(&ids).unwrap_or_default();
        if let Ok(projects) = client.get(format!("{MODRINTH_API}/projects")).query(&[("ids", ids.as_str())]).send().await {
            if let Ok(list) = projects.json::<Vec<Value>>().await {
                for u in &mut updates {
                    if let Some(p) = list.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(u.project_id.as_str())) {
                        u.title = p.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
                    }
                }
            }
        }
    }
    Ok(UpdateReport { known: current.len(), unknown: by_hash.len() - current.len(), updates })
}

/// Replaces `file` with the primary file of Modrinth version `version_id` (sha1-checked).
/// A disabled mod stays disabled. Returns the new file name.
pub async fn update(instance_id: &str, file: &str, version_id: &str) -> Result<String, String> {
    check_name(file)?;
    let dir = game::instance_dir(instance_id).join("mods");
    let old = dir.join(file);
    if !old.is_file() {
        return Err(format!("Файл {file} не найден"));
    }
    let client = game::http_client()?;
    let version: Value = client
        .get(format!("{MODRINTH_API}/version/{version_id}"))
        .send()
        .await
        .map_err(|e| format!("Modrinth недоступен: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Modrinth: {e}"))?;
    let remote = primary_file(&version).ok_or("У новой версии нет файла для скачивания")?;
    let sha1 = version
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| files.iter().find(|f| f.get("filename").and_then(Value::as_str) == Some(remote.filename.as_str())))
        .and_then(|f| f.pointer("/hashes/sha1"))
        .and_then(Value::as_str)
        .ok_or("У новой версии нет sha1")?
        .to_string();
    let disabled = file.ends_with(".disabled");
    let name = if disabled { format!("{}.disabled", remote.filename) } else { remote.filename.clone() };
    let fresh = dir.join(&name);
    // Same name as the old file: download next to it first, so a failed download keeps the mod.
    let staging = if fresh == old { dir.join(format!("{name}.new")) } else { fresh.clone() };
    crate::mrpack::download_verified(&client, &remote.url, &staging, &sha1, remote.size).await?;
    if staging != fresh {
        fs::rename(&staging, &fresh).map_err(|e| format!("{}: {e}", fresh.display()))?;
    } else {
        remove_file(&old)?;
    }
    Ok(name)
}

/// Modrinth loader name and game version of an installation (from instance.json).
fn instance_target(instance_id: &str) -> Result<(&'static str, String), String> {
    let meta = game::read_json(&game::instance_dir(instance_id).join("instance.json"))?;
    let label = meta.get("version").and_then(Value::as_str).unwrap_or("");
    let (loader, version) = game::parse_label(label);
    let version = version.ok_or("Не удалось определить версию игры установки")?;
    let loader = match loader {
        Loader::Fabric => "fabric",
        Loader::Forge => "forge",
        Loader::NeoForge => "neoforge",
        Loader::Vanilla => return Err("В ванильной установке нет модов для обновления".into()),
    };
    Ok((loader, version))
}

async fn modrinth_post(client: &reqwest::Client, path: &str, body: Value) -> Result<serde_json::Map<String, Value>, String> {
    let response = client
        .post(format!("{MODRINTH_API}{path}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Modrinth недоступен: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Modrinth: HTTP {}", response.status().as_u16()));
    }
    response.json().await.map_err(|e| format!("Modrinth: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Hits the real Modrinth API: `cargo test -- --ignored live_modrinth --nocapture`
    #[test]
    #[ignore]
    fn live_modrinth() {
        tauri::async_runtime::block_on(async {
            let client = game::http_client().unwrap();
            for (name, loader, gv) in [("Just Enough Items (JEI)", "fabric", "1.21.1"), ("Sodium", "fabric", "1.21.1"), ("Create", "neoforge", "1.21.1")] {
                let id = search_project(&client, name, loader, gv).await.unwrap();
                println!("{name} → {id:?}");
                let id = id.expect("project found");
                let version = pick_version(&client, &id, loader, gv).await.unwrap().expect("version found");
                let file = primary_file(&version).expect("file");
                let deps: Vec<_> = version["dependencies"].as_array().unwrap().iter()
                    .filter(|d| d["dependency_type"] == "required").map(|d| d["project_id"].clone()).collect();
                println!("   {} ({:?} bytes), required deps: {deps:?}", file.filename, file.size);
            }
        });
    }

    #[test]
    fn toggles_disabled_suffix() {
        assert_eq!(toggled_name("sodium.jar", false), "sodium.jar.disabled");
        assert_eq!(toggled_name("sodium.jar.disabled", true), "sodium.jar");
        assert_eq!(toggled_name("sodium.jar", true), "sodium.jar");
        assert_eq!(toggled_name("sodium.jar.disabled", false), "sodium.jar.disabled");
    }

    #[test]
    fn cleans_catalog_titles() {
        assert_eq!(clean_query("Just Enough Items (JEI)"), "Just Enough Items");
        assert_eq!(clean_query("Jade 🔍"), "Jade");
        assert_eq!(clean_query("Xaero's Minimap"), "Xaero's Minimap");
    }

    #[test]
    fn picks_primary_file_and_rejects_paths() {
        let v = json!({ "files": [
            { "url": "https://cdn/a.jar", "filename": "a-sources.jar", "primary": false },
            { "url": "https://cdn/b.jar", "filename": "b.jar", "primary": true, "size": 10 }
        ]});
        let f = primary_file(&v).unwrap();
        assert_eq!(f.filename, "b.jar");
        assert_eq!(f.size, Some(10));
        let bad = json!({ "files": [{ "url": "u", "filename": "../evil.jar", "primary": true }] });
        assert!(primary_file(&bad).is_none());
    }

    /// Read-only: which mods of the real installations have updates.
    /// `cargo test --lib live_updates -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_updates() {
        for inst in crate::instances::list().into_iter().filter(|i| i.mods > 0) {
            let report = tauri::async_runtime::block_on(check_updates(&inst.id));
            match report {
                Ok(r) => {
                    println!("«{}» ({}): known {}, unknown {}, updates {}", inst.name, inst.version, r.known, r.unknown, r.updates.len());
                    for u in r.updates.iter().take(8) {
                        println!("  {} [{}]: {} -> {}", u.title, u.file, u.current, u.latest);
                    }
                }
                Err(e) => println!("«{}»: {e}", inst.name),
            }
        }
    }
}
