//! Installations are folders `.minecraft/instances/<id>` with an `instance.json`:
//! `{ id, name, version, created, lastPlayed, playTime, lastBackup, lastExitCode }`.
//! The launcher shows exactly what is on disk.

use std::fs;

use serde::Serialize;
use serde_json::Value;

use crate::game;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    pub id: String,
    pub name: String,
    /// Launch label, e.g. `Release 26.3` or `Fabric 1.21.1`.
    pub version: String,
    pub created: Option<i64>,
    pub last_played: Option<i64>,
    /// Seconds in game, summed over all sessions.
    pub play_time: u64,
    pub last_backup: Option<i64>,
    pub last_exit_code: Option<i64>,
    /// Enabled `.jar` files in `mods/`.
    pub mods: usize,
    pub worlds: usize,
    /// `icon.png|webp|jpg|gif` in the installation folder (imported modpacks).
    pub icon: Option<String>,
    /// Modpack name and version for imported .mrpack installations.
    pub pack: Option<String>,
    pub loader_version: Option<String>,
    /// The installation's own launch settings: `{ custom, ramGb, java, jvmArgs }` (null — global ones).
    pub launch: Option<Value>,
}

/// Every installation with a readable `instance.json`, most recently played first.
pub fn list() -> Vec<InstanceInfo> {
    let Ok(rd) = fs::read_dir(game::minecraft_dir().join("instances")) else {
        return Vec::new();
    };
    let mut out: Vec<InstanceInfo> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let dir = e.path();
            let meta = game::read_json(&dir.join("instance.json")).ok()?;
            let id = dir.file_name()?.to_str()?.to_string();
            let text = |key: &str| meta.get(key).and_then(Value::as_str).map(str::to_string).filter(|s| !s.trim().is_empty());
            let int = |key: &str| meta.get(key).and_then(Value::as_i64);
            Some(InstanceInfo {
                name: text("name").unwrap_or_else(|| id.clone()),
                version: text("version")?,
                created: int("created"),
                last_played: int("lastPlayed"),
                play_time: meta.get("playTime").and_then(Value::as_u64).unwrap_or(0),
                last_backup: int("lastBackup"),
                last_exit_code: int("lastExitCode"),
                mods: count(&dir.join("mods"), |name, is_dir| !is_dir && name.ends_with(".jar")),
                worlds: count(&dir.join("saves"), |_, is_dir| is_dir),
                icon: ["png", "webp", "jpg", "jpeg", "gif"]
                    .iter()
                    .map(|ext| dir.join(format!("icon.{ext}")))
                    .find(|p| p.is_file())
                    .map(|p| game::path_str(&p)),
                pack: meta.get("pack").and_then(|p| {
                    let name = p.get("name").and_then(Value::as_str)?;
                    Some(match p.get("version").and_then(Value::as_str) {
                        Some(v) => format!("{name} {v}"),
                        None => name.to_string(),
                    })
                }),
                loader_version: text("loaderVersion"),
                launch: meta.get("launch").cloned(),
                id,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_played
            .unwrap_or(0)
            .cmp(&a.last_played.unwrap_or(0))
            .then(b.created.unwrap_or(0).cmp(&a.created.unwrap_or(0)))
    });
    out
}

/// «Новая установка»: creates the folder and `instance.json` right away, so it is listed
/// before the first launch.
pub fn create(id: &str, name: &str, version: &str) -> Result<(), String> {
    if id.trim().is_empty() || name.trim().is_empty() || version.trim().is_empty() {
        return Err("Нужны id, название и версия установки".into());
    }
    let dir = game::instance_dir(id);
    if dir.join("instance.json").is_file() {
        return Err("Установка с таким id уже есть".into());
    }
    game::write_instance_meta(id, name.trim(), version.trim());
    if dir.join("instance.json").is_file() {
        Ok(())
    } else {
        Err(format!("Не удалось создать {}", dir.display()))
    }
}

/// «Переименовать»: only the display name changes; the folder keeps its id.
pub fn rename(id: &str, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err("Название — от 1 до 40 символов".into());
    }
    existing(id)?;
    game::update_instance_meta(id, |meta| {
        meta.insert("name".into(), name.into());
    });
    Ok(())
}

/// «Дублировать»: a full copy (mods, configs, worlds, resource packs) under a new id, without
/// logs, crash reports and play statistics. Returns the new id.
pub fn duplicate(id: &str, name: &str) -> Result<String, String> {
    let source = existing(id)?;
    let new_id = format!("c{}", game::now_ms());
    let target = game::instance_dir(&new_id);
    copy_tree(&source, &target).map_err(|e| {
        let _ = fs::remove_dir_all(&target);
        format!("Не удалось скопировать установку: {e}")
    })?;
    let name = name.trim().chars().take(40).collect::<String>();
    game::update_instance_meta(&new_id, |meta| {
        meta.insert("id".into(), new_id.clone().into());
        meta.insert("name".into(), name.into());
        meta.insert("created".into(), game::now_ms().into());
        for key in ["lastPlayed", "playTime", "lastExitCode", "lastBackup"] {
            meta.remove(key);
        }
    });
    Ok(new_id)
}

/// «Удалить»: removes `.minecraft/instances/<id>` with everything in it.
pub fn delete(id: &str) -> Result<(), String> {
    let dir = existing(id)?;
    fs::remove_dir_all(&dir).map_err(|e| {
        format!("Не удалось удалить {}: {e}. Если игра этой установки запущена, закройте её.", dir.display())
    })
}

/// The installation folder, if `id` names a real one directly inside `instances/`.
fn existing(id: &str) -> Result<std::path::PathBuf, String> {
    let dir = game::instance_dir(id);
    let inside = dir.parent() == Some(game::minecraft_dir().join("instances").as_path());
    if id.trim().is_empty() || !inside || dir.file_name().and_then(|n| n.to_str()) != Some(id) || !dir.is_dir() {
        return Err("Установка не найдена".into());
    }
    Ok(dir)
}

/// Copies a folder tree, leaving out what belongs to one particular run.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let lower = name.to_string_lossy().to_lowercase();
        if from.join("instance.json").is_file()
            && (lower == "logs" || lower == "crash-reports" || lower == "signal-launch.log" || lower.starts_with("hs_err_pid"))
        {
            continue;
        }
        if lower == "session.lock" {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            copy_tree(&path, &to.join(&name))?;
        } else {
            fs::copy(&path, to.join(&name))?;
        }
    }
    Ok(())
}

fn count(dir: &std::path::Path, keep: impl Fn(&str, bool) -> bool) -> usize {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    keep(&name, e.path().is_dir())
                })
                .count()
        })
        .unwrap_or(0)
}

/// «Параметры запуска» of an installation, stored in its `instance.json` as `launch`.
/// The installation may not exist yet (a built-in one never launched): it is created.
pub fn set_launch(id: &str, name: &str, version: &str, custom: bool, ram_gb: u32, java: &str, jvm_args: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("Не выбрана установка".into());
    }
    let dir = game::instance_dir(id);
    let fresh = !dir.join("instance.json").is_file();
    game::update_instance_meta(id, |meta| {
        if fresh {
            meta.insert("id".into(), id.into());
            meta.insert("name".into(), name.into());
            meta.insert("version".into(), version.into());
        }
        meta.insert(
            "launch".into(),
            serde_json::json!({ "custom": custom, "ramGb": ram_gb.clamp(1, 64), "java": java.trim(), "jvmArgs": jvm_args.trim() }),
        );
    });
    if dir.join("instance.json").is_file() { Ok(()) } else { Err("Не удалось сохранить instance.json".into()) }
}

#[cfg(test)]
mod tests {
    /// Reads the real `.minecraft`: `cargo test -- --ignored live_instances --nocapture`
    #[test]
    #[ignore]
    fn live_instances() {
        for i in super::list() {
            println!("{} · «{}» · {} · mods {} · worlds {} · last {:?} · played {}s", i.id, i.name, i.version, i.mods, i.worlds, i.last_played, i.play_time);
        }
    }
}
