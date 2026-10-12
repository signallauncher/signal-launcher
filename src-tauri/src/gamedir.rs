//! The game folder (`.minecraft` by default) can live anywhere: «Сменить папку…».
//!
//! The choice is stored in `<app data>/settings.json`, so it survives updates and does not
//! depend on the folder itself. Three ways to switch:
//! - `use`  — point the launcher at a folder as is (another `.minecraft`, an empty folder);
//! - `copy` — copy everything into an empty folder and switch, the old one stays;
//! - `move` — the same, then the old folder is removed (fast rename on the same disk).
//!   Not offered for the default `%APPDATA%\.minecraft`: the official launcher uses it too.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::game::Progress;

static OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);
/// Games started by this launcher and still running (the folder must not move under them).
pub static RUNNING_GAMES: AtomicUsize = AtomicUsize::new(0);

pub fn default_dir() -> PathBuf {
    #[cfg(windows)]
    {
        crate::expand_path(r"%APPDATA%\.minecraft")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/minecraft")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".minecraft")
    }
}

pub fn current() -> PathBuf {
    OVERRIDE.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(default_dir)
}

fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("settings.json"))
}

/// At start: the saved folder, if it still exists; the asset protocol may read it.
pub fn init(app: &AppHandle) {
    let saved = settings_path(app)
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("gameDir").and_then(Value::as_str).map(PathBuf::from))
        .filter(|p| p.is_dir());
    if let Some(dir) = saved {
        let _ = app.asset_protocol_scope().allow_directory(&dir, true);
        *OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = Some(dir);
    }
}

fn save(app: &AppHandle, dir: Option<&Path>) -> Result<(), String> {
    let path = settings_path(app).ok_or("Нет папки данных приложения")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut settings = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Map<String, Value>>(&b).ok())
        .unwrap_or_default();
    match dir {
        Some(d) => settings.insert("gameDir".into(), crate::game::path_str(d).into()),
        None => settings.remove("gameDir"),
    };
    fs::write(&path, Value::Object(settings).to_string()).map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameDirInfo {
    pub path: String,
    pub default_path: String,
    pub is_default: bool,
}

pub fn info() -> GameDirInfo {
    let path = current();
    let default = default_dir();
    GameDirInfo { is_default: path == default, path: crate::game::path_str(&path), default_path: crate::game::path_str(&default) }
}

/// What a candidate folder looks like, for the dialog: empty, a Minecraft folder, or other files.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCheck {
    pub exists: bool,
    pub empty: bool,
    pub minecraft: bool,
    pub same: bool,
}

pub fn check(path: &str) -> FolderCheck {
    let p = PathBuf::from(path.trim());
    let entries: Vec<String> = fs::read_dir(&p)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_lowercase()).collect())
        .unwrap_or_default();
    FolderCheck {
        exists: p.is_dir(),
        empty: entries.is_empty(),
        minecraft: ["versions", "libraries", "assets", "instances", "launcher_profiles.json"].iter().any(|n| entries.iter().any(|e| e == n)),
        same: same_path(&p, &current()),
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Switches the game folder. Progress (copy/move): `gamedir-progress`.
pub fn change(app: &AppHandle, path: &str, mode: &str) -> Result<GameDirInfo, String> {
    if RUNNING_GAMES.load(Ordering::SeqCst) > 0 {
        return Err("Сначала закройте игру: папку нельзя менять, пока Minecraft запущен.".into());
    }
    let target = PathBuf::from(path.trim());
    if !target.is_absolute() {
        return Err("Нужен полный путь к папке".into());
    }
    let old = current();
    if same_path(&target, &old) {
        return Err("Это уже текущая папка игры".into());
    }
    match mode {
        "use" => {
            fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        "copy" | "move" => {
            if target.starts_with(&old) || old.starts_with(&target) {
                return Err("Новая папка не может быть внутри старой (и наоборот)".into());
            }
            let c = check(path);
            if c.exists && !c.empty {
                return Err("Для переноса нужна пустая папка. Чтобы просто открыть другую папку игры, выберите «Использовать как есть».".into());
            }
            if mode == "move" && same_path(&old, &default_dir()) {
                return Err("Стандартную .minecraft переносить нельзя: ей пользуется и официальный лаунчер. Выберите «Скопировать».".into());
            }
            let progress = Progress::new(app.clone(), "gamedir-progress");
            // Same disk: one rename instead of copying gigabytes.
            let renamed = mode == "move" && !target.exists() && fs::rename(&old, &target).is_ok();
            if !renamed {
                copy_all(&old, &target, &progress).map_err(|e| {
                    let _ = fs::remove_dir_all(&target);
                    format!("Не удалось скопировать: {e}")
                })?;
                if mode == "move" {
                    if let Err(e) = fs::remove_dir_all(&old) {
                        progress.step("done", format!("Скопировано, но старая папка не удалилась: {e}"), 100.0);
                    }
                }
            }
            progress.step("done", "Готово", 100.0);
        }
        _ => return Err("Неизвестный режим".into()),
    }
    let is_default = same_path(&target, &default_dir());
    save(app, (!is_default).then_some(target.as_path()))?;
    let _ = app.asset_protocol_scope().allow_directory(&target, true);
    *OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = (!is_default).then_some(target);
    Ok(info())
}

fn copy_all(from: &Path, to: &Path, progress: &Progress) -> std::io::Result<()> {
    let total = count_files(from);
    let mut done = 0usize;
    copy_rec(from, to, progress, &mut done, total)
}

fn count_files(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => count_files(&e.path()),
                    Ok(t) if t.is_file() => 1,
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

fn copy_rec(from: &Path, to: &Path, progress: &Progress, done: &mut usize, total: usize) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_rec(&entry.path(), &dest, progress, done, total)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &dest)?;
            *done += 1;
            progress.report(
                "copy",
                format!("Копирование файлов ({done}/{total})…"),
                100.0 * *done as f64 / total.max(1) as f64,
                *done,
                total,
                *done == total,
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Disk usage
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePart {
    /// `versions`, `libraries`, `assets`, `runtime`, `instances`, `backups`, `cache`, `other`
    pub key: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    pub path: String,
    pub total: u64,
    pub parts: Vec<UsagePart>,
    /// Free space on the disk the folder is on.
    pub free: Option<u64>,
}

pub fn usage() -> DiskUsage {
    let base = current();
    let mut parts: Vec<UsagePart> = Vec::new();
    let mut counted = 0u64;
    for (key, subs) in [
        ("versions", &["versions"][..]),
        ("libraries", &["libraries"][..]),
        ("assets", &["assets"][..]),
        ("runtime", &["runtime"][..]),
        ("instances", &["instances"][..]),
        ("backups", &["backups"][..]),
        ("cache", &["signal-cache", "signal-installers"][..]),
    ] {
        let bytes: u64 = subs.iter().map(|s| dir_size(&base.join(s))).sum();
        counted += bytes;
        parts.push(UsagePart { key: key.into(), bytes });
    }
    let total = dir_size(&base);
    parts.push(UsagePart { key: "other".into(), bytes: total.saturating_sub(counted) });
    DiskUsage { path: crate::game::path_str(&base), total, parts, free: free_space(&base) }
}

fn dir_size(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => dir_size(&e.path()),
                    Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

fn free_space(path: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path = crate::game::path_str(&path).trim_start_matches(r"\\?\").to_lowercase();
    disks
        .list()
        .iter()
        .filter(|d| path.starts_with(&d.mount_point().to_string_lossy().to_lowercase()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_copies_a_tree() {
        let base = std::env::temp_dir().join(format!("signal-gamedir-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let src = base.join("src");
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("a/b/x.bin"), vec![0u8; 1000]).unwrap();
        fs::write(src.join("y.txt"), b"12345").unwrap();
        assert_eq!(dir_size(&src), 1005);
        assert_eq!(count_files(&src), 2);
        let dst = base.join("dst");
        copy_all(&src, &dst, &Progress::console()).unwrap();
        assert_eq!(dir_size(&dst), 1005);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn reports_free_space_for_temp() {
        assert!(free_space(&std::env::temp_dir()).is_some_and(|b| b > 0));
    }

    /// Read-only: `cargo test --lib live_usage -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_usage() {
        let u = usage();
        println!("{} total {} MB, free {:?} GB", u.path, u.total / 1_048_576, u.free.map(|f| f / 1_073_741_824));
        for p in &u.parts {
            println!("  {:<10} {:>8} MB", p.key, p.bytes / 1_048_576);
        }
    }
}
