//! «Экспорт в .mrpack»: an installation becomes a Modrinth modpack.
//!
//! Mods, resource packs and shaders Modrinth knows (matched by sha1) go into `files` of
//! `modrinth.index.json` with both hashes, the download link and client/server sides taken from the
//! project — so a server using the pack won't try to install a client-only mod. Everything else that
//! was picked (configs, options.txt, own mods, worlds…) is packed under `overrides/`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::game::{self, Loader, Progress};

const API: &str = "https://api.modrinth.com/v2";
/// Never offered: belong to one run or one computer.
const SKIP: [&str; 11] = [
    "logs", "crash-reports", "screenshots", "signal-launch.log", "instance.json", "usercache.json",
    "usernamecache.json", ".fabric", ".mixin.out", "debug", "command_history.txt",
];
const CONTENT: [&str; 3] = ["mods", "resourcepacks", "shaderpacks"];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEntry {
    /// Relative path with `/`.
    pub path: String,
    /// `mods`, `resourcepacks`, `shaderpacks`, `config`, `saves`, `other`
    pub group: String,
    pub size: u64,
    pub is_dir: bool,
    pub selected: bool,
    /// Found on Modrinth: goes into the index as a download, not into the archive.
    pub modrinth: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    pub name: String,
    pub version: String,
    pub loader_version: Option<String>,
    pub entries: Vec<ExportEntry>,
}

pub async fn preview(instance_id: &str) -> Result<ExportPreview, String> {
    let inst = game::instance_dir(instance_id);
    let meta = game::read_json(&inst.join("instance.json"))?;
    let name = meta.get("name").and_then(Value::as_str).unwrap_or(instance_id).to_string();
    let label = meta.get("version").and_then(Value::as_str).unwrap_or_default().to_string();
    let loader_version = meta
        .get("loaderVersion")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| game::installed_loader_version(&game::minecraft_dir(), &label));

    let mut entries = Vec::new();
    let mut top: Vec<(String, bool)> = fs::read_dir(&inst)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.file_type().ok()?.is_dir())))
        .collect();
    top.sort();
    for (item, is_dir) in top {
        let lower = item.to_lowercase();
        if SKIP.contains(&lower.as_str()) || lower.starts_with("hs_err_pid") || lower.starts_with("icon.") {
            continue;
        }
        let path = inst.join(&item);
        if is_dir && CONTENT.contains(&lower.as_str()) {
            for (child, child_dir) in children(&path) {
                // The launcher's personal skin pack is not part of a modpack.
                if lower == "resourcepacks" && child == crate::skinpack::PACK {
                    continue;
                }
                let child_lower = child.to_lowercase();
                let enabled = !child_lower.ends_with(".disabled");
                entries.push(ExportEntry {
                    path: format!("{item}/{child}"),
                    group: lower.clone(),
                    size: if child_dir { dir_size(&path.join(&child)) } else { file_size(&path.join(&child)) },
                    is_dir: child_dir,
                    selected: enabled,
                    modrinth: false,
                });
            }
        } else if is_dir && lower == "saves" {
            for (world, world_dir) in children(&path) {
                if world_dir {
                    entries.push(ExportEntry { path: format!("saves/{world}"), group: "saves".into(), size: dir_size(&path.join(&world)), is_dir: true, selected: false, modrinth: false });
                }
            }
        } else {
            let group = if lower == "config" || lower == "defaultconfigs" || lower == "options.txt" || lower == "optionsof.txt" || lower == "optionsshaders.txt" { "config" } else { "other" };
            entries.push(ExportEntry {
                path: item.clone(),
                group: group.into(),
                size: if is_dir { dir_size(&path) } else { file_size(&path) },
                is_dir,
                // Personal (server list) and machine-local (caches, downloaded server packs) stays out
                // by default; everything else is usually part of a pack.
                selected: !lower.starts_with('.') && !["servers.dat", "servers.dat_old", "downloads"].contains(&lower.as_str()),
                modrinth: false,
            });
        }
    }

    // Which content files Modrinth knows.
    let hashed: Vec<(usize, String)> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| CONTENT.contains(&e.group.as_str()) && !e.is_dir)
        .filter_map(|(i, e)| Some((i, crate::mrpack::sha1_hex(&fs::read(inst.join(&e.path)).ok()?))))
        .collect();
    if !hashed.is_empty() {
        let client = game::http_client()?;
        if let Ok(found) = version_files(&client, &hashed.iter().map(|(_, h)| h.clone()).collect::<Vec<_>>()).await {
            for (i, hash) in &hashed {
                entries[*i].modrinth = found.contains_key(hash);
            }
        }
    }
    Ok(ExportPreview { name, version: label, loader_version, entries })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub author: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub path: String,
    /// Files listed in the index (downloaded from Modrinth on import).
    pub downloads: usize,
    /// Files packed under overrides/.
    pub overrides: usize,
    pub size: u64,
}

/// Writes the pack to `dest` (`.mrpack` is added if missing). Progress: `export-progress`.
pub async fn export(app: AppHandle, instance_id: &str, dest: &str, meta: ExportMeta, include: Vec<String>) -> Result<ExportReport, String> {
    export_with(&Progress::new(app, "export-progress"), instance_id, dest, meta, include).await
}

async fn export_with(progress: &Progress, instance_id: &str, dest: &str, meta: ExportMeta, include: Vec<String>) -> Result<ExportReport, String> {
    let inst = game::instance_dir(instance_id);
    let info = game::read_json(&inst.join("instance.json"))?;
    let label = info.get("version").and_then(Value::as_str).unwrap_or_default().to_string();
    let (loader, mc) = game::parse_label(&label);
    let mc = mc.ok_or("Не удалось определить версию игры установки")?;
    let mut dependencies = serde_json::Map::new();
    dependencies.insert("minecraft".into(), mc.clone().into());
    if loader != Loader::Vanilla {
        let lv = info
            .get("loaderVersion")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| game::installed_loader_version(&game::minecraft_dir(), &label))
            .ok_or("Версия загрузчика ещё неизвестна: запустите эту установку один раз, и экспорт заработает.")?;
        let key = match loader {
            Loader::Fabric => "fabric-loader",
            Loader::Forge => "forge",
            _ => "neoforge",
        };
        dependencies.insert(key.into(), lv.into());
    }
    if meta.name.trim().is_empty() || meta.version.trim().is_empty() {
        return Err("Укажите название и версию сборки".into());
    }

    // Only paths inside the installation, as offered by the preview.
    let mut picked: Vec<PathBuf> = Vec::new();
    for rel in &include {
        let rel = crate::mrpack::safe_relative(rel).ok_or_else(|| format!("Недопустимый путь: {rel}"))?;
        if inst.join(&rel).exists() {
            picked.push(rel);
        }
    }

    progress.step("hash", "Сверка файлов с Modrinth…", 2.0);
    let content: Vec<(PathBuf, String)> = picked
        .iter()
        .filter(|rel| {
            let first = rel.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).unwrap_or_default();
            CONTENT.contains(&first.as_str()) && inst.join(rel).is_file()
        })
        .filter_map(|rel| Some((rel.clone(), crate::mrpack::sha1_hex(&fs::read(inst.join(rel)).ok()?))))
        .collect();
    let client = game::http_client()?;
    let found = if content.is_empty() {
        HashMap::new()
    } else {
        version_files(&client, &content.iter().map(|(_, h)| h.clone()).collect::<Vec<_>>()).await?
    };
    let project_ids: Vec<String> = found.values().filter_map(|v| v.get("project_id").and_then(Value::as_str).map(str::to_string)).collect();
    let sides = project_sides(&client, &project_ids).await.unwrap_or_default();

    let mut files = Vec::new();
    let mut downloads: Vec<PathBuf> = Vec::new();
    for (rel, sha1) in &content {
        let Some(version) = found.get(sha1) else { continue };
        let Some(file) = version
            .get("files")
            .and_then(Value::as_array)
            .and_then(|fs| fs.iter().find(|f| f.pointer("/hashes/sha1").and_then(Value::as_str) == Some(sha1.as_str())))
        else {
            continue;
        };
        let (Some(url), Some(sha512)) = (file.get("url").and_then(Value::as_str), file.pointer("/hashes/sha512").and_then(Value::as_str)) else { continue };
        let project = version.get("project_id").and_then(Value::as_str).unwrap_or_default();
        let is_mod = rel.starts_with("mods");
        let (client_side, server_side) = sides.get(project).cloned().unwrap_or(("required".into(), "required".into()));
        files.push(json!({
            "path": rel.to_string_lossy().replace('\\', "/"),
            "hashes": { "sha1": sha1, "sha512": sha512 },
            "env": {
                "client": side(&client_side, "required"),
                // Resource packs and shaders never belong on a server.
                "server": if is_mod { side(&server_side, "optional") } else { "unsupported".into() }
            },
            "downloads": [url],
            "fileSize": file.get("size").and_then(Value::as_u64).unwrap_or_else(|| file_size(&inst.join(rel)))
        }));
        downloads.push(rel.clone());
    }

    let mut summary = meta.summary.trim().to_string();
    if !meta.author.trim().is_empty() {
        summary = if summary.is_empty() { format!("by {}", meta.author.trim()) } else { format!("{summary} — by {}", meta.author.trim()) };
    }
    let mut index = json!({
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": meta.version.trim(),
        "name": meta.name.trim(),
        "files": files,
        "dependencies": dependencies,
    });
    if !summary.is_empty() {
        index["summary"] = summary.into();
    }

    let mut dest = PathBuf::from(dest.trim());
    if dest.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() != Some("mrpack") {
        dest.set_extension("mrpack");
    }
    let overrides: Vec<PathBuf> = picked.into_iter().filter(|p| !downloads.contains(p)).collect();
    let total_files: usize = overrides.iter().map(|p| count_files(&inst.join(p))).sum();
    let tmp = dest.with_extension("mrpack.part");
    let written = (|| -> Result<usize, String> {
        let mut zip = zip::ZipWriter::new(File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("modrinth.index.json", opts).map_err(|e| e.to_string())?;
        zip.write_all(serde_json::to_string_pretty(&index).unwrap_or_default().as_bytes()).map_err(|e| e.to_string())?;
        let mut done = 0usize;
        for rel in &overrides {
            add_tree(&mut zip, opts, &inst, rel, progress, &mut done, total_files)?;
        }
        zip.finish().map_err(|e| e.to_string())?;
        Ok(done)
    })();
    let packed = match written {
        Ok(n) => n,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(format!("Не удалось записать сборку: {e}"));
        }
    };
    fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    progress.step("done", "Готово", 100.0);
    Ok(ExportReport { path: game::path_str(&dest), downloads: downloads.len(), overrides: packed, size: file_size(&dest) })
}

fn side(value: &str, unknown: &str) -> String {
    match value {
        "required" | "optional" | "unsupported" => value.into(),
        _ => unknown.into(),
    }
}

/// sha1 → Modrinth version for the files it knows.
async fn version_files(client: &reqwest::Client, hashes: &[String]) -> Result<HashMap<String, Value>, String> {
    let response = client
        .post(format!("{API}/version_files"))
        .json(&json!({ "hashes": hashes, "algorithm": "sha1" }))
        .send()
        .await
        .map_err(|e| format!("Modrinth недоступен: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Modrinth: HTTP {}", response.status().as_u16()));
    }
    let map: serde_json::Map<String, Value> = response.json().await.map_err(|e| format!("Modrinth: {e}"))?;
    Ok(map.into_iter().collect())
}

/// project id → (client_side, server_side)
async fn project_sides(client: &reqwest::Client, ids: &[String]) -> Result<HashMap<String, (String, String)>, String> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let list: Vec<Value> = client
        .get(format!("{API}/projects"))
        .query(&[("ids", serde_json::to_string(ids).unwrap_or_default())])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("unknown").to_string();
    Ok(list.iter().map(|p| (text(p, "id"), (text(p, "client_side"), text(p, "server_side")))).collect())
}

fn add_tree(
    zip: &mut zip::ZipWriter<File>,
    opts: zip::write::SimpleFileOptions,
    base: &Path,
    rel: &Path,
    progress: &Progress,
    done: &mut usize,
    total: usize,
) -> Result<(), String> {
    let path = base.join(rel);
    let name = format!("overrides/{}", rel.to_string_lossy().replace('\\', "/"));
    if path.is_dir() {
        zip.add_directory(format!("{name}/"), opts).map_err(|e| e.to_string())?;
        for (child, _) in children(&path) {
            if child.eq_ignore_ascii_case("session.lock") {
                continue;
            }
            add_tree(zip, opts, base, &rel.join(&child), progress, done, total)?;
        }
    } else {
        let mut buf = Vec::new();
        File::open(&path).and_then(|mut f| f.read_to_end(&mut buf)).map_err(|e| format!("{}: {e}", path.display()))?;
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(&buf).map_err(|e| e.to_string())?;
        *done += 1;
        progress.report("pack", format!("Упаковка файлов ({done}/{total})…"), 5.0 + 93.0 * *done as f64 / total.max(1) as f64, *done, total, *done == total);
    }
    Ok(())
}

fn children(dir: &Path) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| Some((e.file_name().into_string().ok()?, e.file_type().ok()?.is_dir()))).collect())
        .unwrap_or_default();
    out.sort_by_key(|(n, _)| n.to_lowercase());
    out
}

fn file_size(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn dir_size(dir: &Path) -> u64 {
    children(dir).iter().map(|(n, d)| if *d { dir_size(&dir.join(n)) } else { file_size(&dir.join(n)) }).sum()
}

fn count_files(p: &Path) -> usize {
    if p.is_dir() {
        children(p).iter().map(|(n, _)| count_files(&p.join(n))).sum()
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_fall_back() {
        assert_eq!(side("unsupported", "optional"), "unsupported");
        assert_eq!(side("unknown", "optional"), "optional");
    }

    /// The first real installation with mods → .mrpack in a temp folder → read back.
    /// `cargo test --lib live_export -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_export() {
        let inst = crate::instances::list().into_iter().find(|i| i.mods > 0).expect("an installation with mods");
        let preview = tauri::async_runtime::block_on(preview(&inst.id)).unwrap();
        println!("«{}» {} loader {:?}: {} entries, {} on Modrinth", preview.name, preview.version, preview.loader_version, preview.entries.len(), preview.entries.iter().filter(|e| e.modrinth).count());
        for e in preview.entries.iter().filter(|e| !e.modrinth).take(8) {
            println!("  overrides? {} [{}] selected={}", e.path, e.group, e.selected);
        }
        let include: Vec<String> = preview.entries.iter().filter(|e| e.selected).map(|e| e.path.clone()).collect();
        let dest = std::env::temp_dir().join("signal-export-live.mrpack");
        let meta = ExportMeta { name: preview.name.clone(), version: "1.0.0".into(), summary: "test".into(), author: "Signal".into() };
        let report = tauri::async_runtime::block_on(export_with(&Progress::console(), &inst.id, &crate::game::path_str(&dest), meta, include));
        println!("{report:?}");
        // Round trip: the pack must import with Signal's own importer.
        let back = std::env::temp_dir().join("signal-export-roundtrip");
        let _ = fs::remove_dir_all(&back);
        let (index, info, counts) = tauri::async_runtime::block_on(crate::mrpack::unpack(&Progress::console(), &dest, &back)).unwrap();
        println!("re-imported «{}» as {} (loader {:?}): {} downloads, {} overrides", index.name, info.label, info.loader_version, counts.0, counts.2);
        let _ = fs::remove_dir_all(&back);
        let _ = fs::remove_file(&dest);
    }
}
