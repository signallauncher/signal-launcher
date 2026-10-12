//! Modrinth modpacks (`.mrpack`): a zip with `modrinth.index.json` (game version, loader,
//! files with hashes and download links) and `overrides/` / `client-overrides/` folders that
//! are copied into the installation as is.
//!
//! The pack decides where its files go and where they come from, so every path is checked to
//! stay inside the installation, downloads come only from the hosts the format allows, and
//! every file must match its sha1.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use tauri::AppHandle;

use crate::game::{self, Progress};

const MODRINTH_API: &str = "https://api.modrinth.com/v2";
/// Download hosts allowed by the .mrpack format.
const ALLOWED_HOSTS: [&str; 4] = ["cdn.modrinth.com", "github.com", "raw.githubusercontent.com", "gitlab.com"];
const PARALLEL: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Index {
    format_version: u32,
    game: String,
    version_id: String,
    pub(crate) name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    files: Vec<PackFile>,
    #[serde(default)]
    dependencies: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackFile {
    path: String,
    hashes: std::collections::HashMap<String, String>,
    #[serde(default)]
    env: Option<Env>,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default)]
    file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Env {
    #[serde(default)]
    client: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub instance_id: String,
    pub name: String,
    /// Launch label, e.g. `Fabric 26.3`.
    pub version: String,
    pub loader_version: Option<String>,
    /// Downloaded files (mods, resource packs, shaders…).
    pub files: usize,
    /// Files marked server-only, skipped.
    pub server_only: usize,
    /// Files copied from overrides/.
    pub overrides: usize,
    pub icon: Option<String>,
}

/// Parsed pack: what the installation will be.
#[derive(Debug, PartialEq)]
pub(crate) struct PackInfo {
    pub(crate) label: String,
    pub(crate) loader_version: Option<String>,
}

/// «Импорт сборки»: unpacks `path` into a new installation `.minecraft/instances/<id>`.
/// Progress: `import-progress`. On any error the half-made installation is removed.
pub async fn import(app: AppHandle, path: String) -> Result<ImportReport, String> {
    let progress = Progress::new(app, "import-progress");
    let pack = PathBuf::from(path.trim());
    if !pack.is_file() || pack.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() != Some("mrpack") {
        return Err("Нужен файл сборки Modrinth с расширением .mrpack".into());
    }
    let id = format!("m{}", game::now_ms());
    let dir = game::instance_dir(&id);
    match unpack(&progress, &pack, &dir).await {
        Ok((index, info, counts)) => {
            let icon = fetch_icon(&pack, &dir).await;
            let title = index.name.trim().to_string();
            game::update_instance_meta(&id, |meta| {
                meta.insert("id".into(), id.clone().into());
                meta.insert("name".into(), title.clone().into());
                meta.insert("version".into(), info.label.clone().into());
                if let Some(lv) = &info.loader_version {
                    meta.insert("loaderVersion".into(), lv.clone().into());
                }
                meta.insert(
                    "pack".into(),
                    serde_json::json!({ "name": title, "version": index.version_id, "summary": index.summary }),
                );
            });
            progress.step("done", "Сборка установлена", 100.0);
            Ok(ImportReport {
                instance_id: id,
                name: title,
                version: info.label,
                loader_version: info.loader_version,
                files: counts.0,
                server_only: counts.1,
                overrides: counts.2,
                icon: icon.map(|p| game::path_str(&p)),
            })
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// Reads the index, downloads the files and copies the overrides into `dir`.
/// Returns (index, installation info, (downloaded, server-only, overrides)).
pub(crate) async fn unpack(progress: &Progress, pack: &Path, dir: &Path) -> Result<(Index, PackInfo, (usize, usize, usize)), String> {
    progress.step("read", "Чтение сборки…", 1.0);
    let index: Index = {
        let mut archive = open_zip(pack)?;
        let mut entry = archive
            .by_name("modrinth.index.json")
            .map_err(|_| "В архиве нет modrinth.index.json — это не сборка Modrinth".to_string())?;
        let mut text = String::new();
        entry.read_to_string(&mut text).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("modrinth.index.json: {e}"))?
    };
    if index.format_version != 1 || index.game != "minecraft" {
        return Err(format!("Неподдерживаемый формат сборки (formatVersion {}, game {})", index.format_version, index.game));
    }
    let info = pack_info(&index.dependencies)?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    // Files to download: everything the client needs (optional ones too).
    let mut jobs = Vec::new();
    let mut server_only = 0usize;
    for file in &index.files {
        if file.env.as_ref().and_then(|e| e.client.as_deref()) == Some("unsupported") {
            server_only += 1;
            continue;
        }
        let rel = safe_relative(&file.path).ok_or_else(|| format!("Недопустимый путь в сборке: {}", file.path))?;
        let sha1 = file.hashes.get("sha1").ok_or_else(|| format!("У файла {} нет sha1", file.path))?.to_lowercase();
        let url = file
            .downloads
            .iter()
            .find(|u| allowed_url(u))
            .ok_or_else(|| format!("Файл {} скачивается с недопустимого сайта", file.path))?
            .clone();
        jobs.push((dir.join(rel), url, sha1, file.path.clone(), file.file_size));
    }

    let total = jobs.len();
    let client = game::http_client()?;
    progress.report("files", format!("Скачивание файлов сборки (0/{total})…"), 3.0, 0, total, true);
    let mut results = Box::pin(
        stream::iter(jobs.into_iter().map(|(dest, url, sha1, name, size)| {
            let client = &client;
            async move { download_verified(client, &url, &dest, &sha1, size).await.map_err(|e| format!("{name}: {e}")) }
        }))
        .buffer_unordered(PARALLEL),
    );
    let mut done = 0usize;
    while let Some(result) = results.next().await {
        result?;
        done += 1;
        progress.report(
            "files",
            format!("Скачивание файлов сборки ({done}/{total})…"),
            3.0 + 87.0 * done as f64 / total.max(1) as f64,
            done,
            total,
            done == total,
        );
    }

    progress.step("overrides", "Копирование настроек сборки (overrides)…", 92.0);
    let pack = pack.to_path_buf();
    let target = dir.to_path_buf();
    let overrides = tauri::async_runtime::spawn_blocking(move || copy_overrides(&pack, &target))
        .await
        .map_err(|e| e.to_string())??;
    Ok((index, info, (total, server_only, overrides)))
}

/// `dependencies` → launch label and pinned loader build.
fn pack_info(deps: &std::collections::HashMap<String, String>) -> Result<PackInfo, String> {
    let mc = deps.get("minecraft").ok_or("В сборке не указана версия Minecraft")?;
    let (label, loader_version) = if let Some(v) = deps.get("fabric-loader") {
        (format!("Fabric {mc}"), Some(v.clone()))
    } else if let Some(v) = deps.get("neoforge") {
        (format!("NeoForge {mc}"), Some(v.clone()))
    } else if let Some(v) = deps.get("forge") {
        (format!("Forge {mc}"), Some(v.clone()))
    } else if deps.contains_key("quilt-loader") {
        return Err("Сборки на Quilt пока не поддерживаются: Signal умеет Fabric, Forge и NeoForge.".into());
    } else {
        let snapshot = mc.contains('w') || mc.contains("-pre") || mc.contains("-rc") || mc.contains("snapshot");
        (format!("{} {mc}", if snapshot { "Snapshot" } else { "Release" }), None)
    };
    Ok(PackInfo { label, loader_version })
}

/// A relative path that stays inside the installation (no `..`, no drive or root).
pub(crate) fn safe_relative(path: &str) -> Option<PathBuf> {
    let p = Path::new(path.trim());
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    ALLOWED_HOSTS.contains(&host)
}

pub(crate) fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `url` to `dest` unless a file with that sha1 is already there; 3 attempts.
pub(crate) async fn download_verified(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    sha1: &str,
    size: Option<u64>,
) -> Result<(), String> {
    if fs::read(dest).map(|b| sha1_hex(&b) == sha1).unwrap_or(false) {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut last = String::new();
    for _ in 0..3 {
        match client.get(url).send().await {
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(bytes) => {
                    if size.is_some_and(|s| s != bytes.len() as u64) || sha1_hex(&bytes) != sha1 {
                        last = "файл не совпал с контрольной суммой".into();
                        continue;
                    }
                    let mut tmp = dest.as_os_str().to_owned();
                    tmp.push(".part");
                    let tmp = PathBuf::from(tmp);
                    fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
                    fs::rename(&tmp, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                    return Ok(());
                }
                Err(e) => last = e.to_string(),
            },
            Ok(r) => last = format!("HTTP {}", r.status().as_u16()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("не скачался ({last})"))
}

fn open_zip(pack: &Path) -> Result<zip::ZipArchive<File>, String> {
    let file = File::open(pack).map_err(|e| format!("{}: {e}", pack.display()))?;
    zip::ZipArchive::new(file).map_err(|e| format!("Повреждённый архив сборки: {e}"))
}

/// `overrides/…` then `client-overrides/…` (which wins) into `dir`. Returns the file count.
fn copy_overrides(pack: &Path, dir: &Path) -> Result<usize, String> {
    let mut archive = open_zip(pack)?;
    let mut count = 0usize;
    for prefix in ["overrides/", "client-overrides/"] {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().replace('\\', "/");
            let Some(rel) = name.strip_prefix(prefix) else { continue };
            if rel.is_empty() {
                continue;
            }
            let rel = safe_relative(rel).ok_or_else(|| format!("Недопустимый путь в overrides: {name}"))?;
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
    }
    Ok(count)
}

/// The pack's project icon from Modrinth (found by the pack file's hash), saved as
/// `icon.<ext>` in the installation. Best effort: packs from elsewhere simply get none.
async fn fetch_icon(pack: &Path, dir: &Path) -> Option<PathBuf> {
    let hash = sha1_hex(&fs::read(pack).ok()?);
    let client = game::http_client().ok()?;
    let version: Value = client
        .get(format!("{MODRINTH_API}/version_file/{hash}?algorithm=sha1"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    let project_id = version.get("project_id")?.as_str()?;
    let project: Value = client.get(format!("{MODRINTH_API}/project/{project_id}")).send().await.ok()?.json().await.ok()?;
    let url = project.get("icon_url")?.as_str()?;
    if !allowed_url(url) {
        return None;
    }
    let ext = url.rsplit('.').next().filter(|e| ["png", "webp", "jpg", "jpeg", "gif"].contains(e)).unwrap_or("png");
    let bytes = client.get(url).send().await.ok()?.bytes().await.ok()?;
    let path = dir.join(format!("icon.{ext}"));
    fs::write(&path, &bytes).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;

    #[test]
    fn rejects_paths_outside_the_installation() {
        assert_eq!(safe_relative("mods/sodium.jar"), Some(PathBuf::from("mods").join("sodium.jar")));
        assert_eq!(safe_relative("./config/a.json"), Some(PathBuf::from("config").join("a.json")));
        assert_eq!(safe_relative("../evil.jar"), None);
        assert_eq!(safe_relative("mods/../../evil.jar"), None);
        assert_eq!(safe_relative("/etc/passwd"), None);
        assert_eq!(safe_relative(r"C:\Windows\x.dll"), None);
        assert_eq!(safe_relative(""), None);
    }

    #[test]
    fn only_allowed_hosts() {
        assert!(allowed_url("https://cdn.modrinth.com/data/x/versions/y/a.jar"));
        assert!(allowed_url("https://github.com/o/r/releases/download/v1/a.jar"));
        assert!(!allowed_url("http://cdn.modrinth.com/a.jar"));
        assert!(!allowed_url("https://cdn.modrinth.com.evil.net/a.jar"));
        assert!(!allowed_url("https://example.com/a.jar"));
    }

    #[test]
    fn reads_loader_from_dependencies() {
        let deps = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
        assert_eq!(
            pack_info(&deps(&[("minecraft", "26.3"), ("fabric-loader", "0.19.5")])).unwrap(),
            PackInfo { label: "Fabric 26.3".into(), loader_version: Some("0.19.5".into()) }
        );
        assert_eq!(pack_info(&deps(&[("minecraft", "1.20.1"), ("forge", "47.3.0")])).unwrap().label, "Forge 1.20.1");
        assert_eq!(pack_info(&deps(&[("minecraft", "1.21.1"), ("neoforge", "21.1.77")])).unwrap().label, "NeoForge 1.21.1");
        assert_eq!(pack_info(&deps(&[("minecraft", "1.21.4")])).unwrap().label, "Release 1.21.4");
        assert!(pack_info(&deps(&[("minecraft", "1.21.1"), ("quilt-loader", "0.26")])).is_err());
    }

    #[test]
    fn copies_overrides_with_client_priority() {
        let base = std::env::temp_dir().join(format!("signal-mrpack-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let pack = base.join("t.mrpack");
        {
            let mut zip = zip::ZipWriter::new(File::create(&pack).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("modrinth.index.json", "{}"),
                ("overrides/config/a.txt", "common"),
                ("overrides/options.txt", "o"),
                ("client-overrides/config/a.txt", "client"),
            ] {
                zip.start_file(name, opts).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        let dir = base.join("inst");
        assert_eq!(copy_overrides(&pack, &dir).unwrap(), 3);
        assert_eq!(fs::read_to_string(dir.join("config").join("a.txt")).unwrap(), "client");
        assert_eq!(fs::read_to_string(dir.join("options.txt")).unwrap(), "o");
        let _ = fs::remove_dir_all(&base);
    }

    /// Real pack: the newest Fabulously Optimized into a temp folder.
    /// `cargo test --lib live_mrpack -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_mrpack() {
        let base = std::env::temp_dir().join("signal-mrpack-live");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let result = tauri::async_runtime::block_on(async {
            let client = game::http_client()?;
            let versions: Vec<Value> = client
                .get(format!("{MODRINTH_API}/project/fabulously-optimized/version"))
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let file = versions[0]["files"].as_array().unwrap().iter().find(|f| f["filename"].as_str().unwrap().ends_with(".mrpack")).unwrap().clone();
            let pack = base.join("pack.mrpack");
            let bytes = client.get(file["url"].as_str().unwrap()).send().await.map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
            fs::write(&pack, &bytes).unwrap();
            let dir = base.join("instance");
            let (index, info, counts) = unpack(&Progress::console(), &pack, &dir).await?;
            let icon = fetch_icon(&pack, &dir).await;
            Ok::<_, String>((index.name, info, counts, icon, dir))
        });
        println!("{result:?}");
        let (_, _, (files, _, overrides), icon, dir) = result.unwrap();
        assert!(files > 0 && overrides > 0);
        assert!(fs::read_dir(dir.join("mods")).unwrap().count() > 0);
        assert!(icon.is_some());
    }
}
