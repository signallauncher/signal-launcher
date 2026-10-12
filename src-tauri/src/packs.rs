//! Resource packs and shaders of an installation, switched on the way the game itself does it:
//! - resource packs: the `resourcePacks:[…]` list in `options.txt` (entries `"file/<name>"`);
//! - shaders: `config/iris.properties` (`shaderPack=<name>`, `enableShaders=true`) — Iris is the
//!   shader loader for Fabric and NeoForge, so only one shader is active at a time.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::game;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Resource,
    Shader,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "resourcepack" => Ok(Kind::Resource),
            "shader" => Ok(Kind::Shader),
            _ => Err("Неизвестный тип пака".into()),
        }
    }

    fn folder(self) -> &'static str {
        match self {
            Kind::Resource => "resourcepacks",
            Kind::Shader => "shaderpacks",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackList {
    pub entries: Vec<PackEntry>,
    pub folder: String,
    /// Shaders: Iris is among the installation's mods (without it shaders do nothing).
    pub iris: bool,
}

pub fn list(instance_id: &str, kind: Kind) -> PackList {
    let inst = game::instance_dir(instance_id);
    let dir = inst.join(kind.folder());
    let active = active_set(&inst, kind);
    let mut entries: Vec<PackEntry> = fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let is_dir = e.file_type().ok()?.is_dir();
                    let lower = name.to_lowercase();
                    // The launcher's own skin pack (skinpack.rs) is managed at launch.
                    if (!is_dir && !lower.ends_with(".zip")) || (kind == Kind::Resource && name == crate::skinpack::PACK) {
                        return None;
                    }
                    let size = if is_dir { dir_size(&e.path()) } else { e.metadata().map(|m| m.len()).unwrap_or(0) };
                    Some(PackEntry { enabled: active.contains(&name), name, size, is_dir })
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by_key(|e| e.name.to_lowercase());
    PackList { entries, folder: game::path_str(&dir), iris: kind == Kind::Shader && has_iris(&inst) }
}

pub fn set_enabled(instance_id: &str, kind: Kind, name: &str, enabled: bool) -> Result<(), String> {
    check_name(name)?;
    let inst = game::instance_dir(instance_id);
    if !inst.join(kind.folder()).join(name).exists() {
        return Err(format!("Пак {name} не найден"));
    }
    match kind {
        Kind::Resource => {
            let mut packs = resource_list(&inst);
            let key = format!("file/{name}");
            packs.retain(|p| p != &key);
            if enabled {
                if !packs.iter().any(|p| p == "vanilla") {
                    packs.insert(0, "vanilla".into());
                }
                // Last in the list = highest priority, as in the game's menu.
                packs.push(key);
            }
            write_option(&inst.join("options.txt"), "resourcePacks", &serde_json::to_string(&packs).unwrap_or_else(|_| "[]".into()))
        }
        Kind::Shader => {
            let file = inst.join("config").join("iris.properties");
            if enabled {
                write_property(&file, "shaderPack", name)?;
                write_property(&file, "enableShaders", "true")
            } else {
                write_property(&file, "enableShaders", "false")
            }
        }
    }
}

pub fn remove(instance_id: &str, kind: Kind, name: &str) -> Result<(), String> {
    check_name(name)?;
    let inst = game::instance_dir(instance_id);
    let path = inst.join(kind.folder()).join(name);
    if active_set(&inst, kind).contains(&name.to_string()) {
        set_enabled(instance_id, kind, name, false)?;
    }
    let result = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
    result.map_err(|e| format!("Не удалось удалить {name}: {e}"))
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("Некорректное имя пака".into());
    }
    Ok(())
}

fn active_set(inst: &Path, kind: Kind) -> Vec<String> {
    match kind {
        Kind::Resource => resource_list(inst).into_iter().filter_map(|p| p.strip_prefix("file/").map(str::to_string)).collect(),
        Kind::Shader => {
            let props = fs::read_to_string(inst.join("config").join("iris.properties")).unwrap_or_default();
            let get = |k: &str| props.lines().find_map(|l| l.strip_prefix(&format!("{k}=")).map(|v| v.trim().to_string()));
            match (get("shaderPack"), get("enableShaders").as_deref()) {
                (Some(pack), Some("false")) => {
                    let _ = pack;
                    Vec::new()
                }
                (Some(pack), _) if !pack.is_empty() => vec![pack],
                _ => Vec::new(),
            }
        }
    }
}

/// `resourcePacks:["vanilla","file/x.zip"]` from options.txt.
fn resource_list(inst: &Path) -> Vec<String> {
    option_list(inst, "resourcePacks")
}

/// A JSON list option from options.txt (`resourcePacks`, `incompatibleResourcePacks`).
pub(crate) fn option_list(inst: &Path, key: &str) -> Vec<String> {
    let prefix = format!("{key}:");
    fs::read_to_string(inst.join("options.txt"))
        .ok()
        .and_then(|text| text.lines().find_map(|l| l.strip_prefix(&prefix).map(str::to_string)))
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
        .unwrap_or_default()
}

/// Replaces (or adds) `key:value` in options.txt, keeping every other line as it was.
pub(crate) fn write_option(file: &PathBuf, key: &str, value: &str) -> Result<(), String> {
    write_line(file, &format!("{key}:"), &format!("{key}:{value}"))
}

/// Replaces (or adds) `key=value` in a .properties file.
fn write_property(file: &PathBuf, key: &str, value: &str) -> Result<(), String> {
    write_line(file, &format!("{key}="), &format!("{key}={value}"))
}

fn write_line(file: &PathBuf, prefix: &str, line: &str) -> Result<(), String> {
    let text = fs::read_to_string(file).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    match lines.iter_mut().find(|l| l.starts_with(prefix)) {
        Some(l) => *l = line.to_string(),
        None => lines.push(line.to_string()),
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(file, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", file.display()))
}

fn has_iris(inst: &Path) -> bool {
    fs::read_dir(inst.join("mods"))
        .map(|rd| {
            rd.filter_map(|e| e.ok()).any(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                n.starts_with("iris") && n.ends_with(".jar")
            })
        })
        .unwrap_or(false)
}

fn dir_size(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => dir_size(&e.path()),
                    _ => e.metadata().map(|m| m.len()).unwrap_or(0),
                })
                .sum()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_options_and_properties_in_place() {
        let dir = std::env::temp_dir().join(format!("signal-packs-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let options = dir.join("options.txt");
        fs::write(&options, "version:3955\nresourcePacks:[\"vanilla\"]\nlang:ru_ru\n").unwrap();
        write_option(&options, "resourcePacks", "[\"vanilla\",\"file/a.zip\"]").unwrap();
        let text = fs::read_to_string(&options).unwrap();
        assert!(text.contains("version:3955") && text.contains("lang:ru_ru"));
        assert!(text.contains("resourcePacks:[\"vanilla\",\"file/a.zip\"]"));
        assert_eq!(resource_list(&dir), vec!["vanilla", "file/a.zip"]);

        let props = dir.join("config").join("iris.properties");
        write_property(&props, "shaderPack", "Complementary.zip").unwrap();
        write_property(&props, "enableShaders", "true").unwrap();
        assert_eq!(active_set(&dir, Kind::Shader), vec!["Complementary.zip"]);
        write_property(&props, "enableShaders", "false").unwrap();
        assert!(active_set(&dir, Kind::Shader).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
