//! Worlds and screenshots read straight from disk: the shared `.minecraft` plus every
//! installation in `.minecraft/instances/*`. Also zips a world into `.minecraft/backups`.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value as Nbt;
use flate2::read::GzDecoder;
use serde::Serialize;

use crate::game;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldInfo {
    pub folder: String,
    pub name: String,
    pub path: String,
    pub instance_id: String,
    pub instance_name: String,
    pub game_type: i32,
    pub hardcore: bool,
    pub difficulty: Option<i32>,
    pub version: Option<String>,
    pub last_played: Option<i64>,
    pub size: u64,
    pub icon: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: Option<u64>,
    pub instance_id: String,
    pub instance_name: String,
}

/// (folder, instance id, instance name): the shared game dir first, then each installation.
fn game_roots() -> Vec<(PathBuf, String, String)> {
    let base = game::minecraft_dir();
    let mut roots = vec![(base.clone(), String::new(), "Общая папка .minecraft".to_string())];
    if let Ok(rd) = fs::read_dir(base.join("instances")) {
        let mut instances: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
        instances.sort();
        for dir in instances {
            let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            let name = game::read_json(&dir.join("instance.json"))
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| id.clone());
            roots.push((dir, id, name));
        }
    }
    roots
}

pub fn list_worlds() -> Vec<WorldInfo> {
    let mut worlds: Vec<WorldInfo> = game_roots().into_iter().flat_map(|(root, id, name)| worlds_in(&root, &id, &name)).collect();
    worlds.sort_by_key(|w| std::cmp::Reverse(w.last_played));
    worlds
}

/// Worlds in `<root>/saves` (root = the shared folder or an installation).
fn worlds_in(root: &Path, instance_id: &str, instance_name: &str) -> Vec<WorldInfo> {
    let mut worlds = Vec::new();
    let Ok(rd) = fs::read_dir(root.join("saves")) else {
        return worlds;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let dir = entry.path();
        let level = dir.join("level.dat");
        if !level.is_file() {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        let data = read_level(&level);
        let get = |key: &str| data.as_ref().and_then(|d| field(d, key));
        let icon = dir.join("icon.png");
        worlds.push(WorldInfo {
            name: get("LevelName").and_then(as_string).filter(|n| !n.is_empty()).unwrap_or_else(|| folder.clone()),
            folder,
            path: game::path_str(&dir),
            instance_id: instance_id.to_string(),
            instance_name: instance_name.to_string(),
            game_type: get("GameType").and_then(as_int).unwrap_or(0) as i32,
            hardcore: get("hardcore").and_then(as_int).unwrap_or(0) != 0,
            difficulty: get("Difficulty").and_then(as_int).map(|d| d as i32),
            version: get("Version").and_then(|v| field(v, "Name")).and_then(as_string),
            last_played: get("LastPlayed").and_then(as_int),
            size: dir_size(&dir),
            icon: icon.is_file().then(|| game::path_str(&icon)),
        });
    }
    worlds
}

pub fn list_screenshots() -> Vec<ScreenshotInfo> {
    let mut shots = Vec::new();
    for (root, instance_id, instance_name) in game_roots() {
        let Ok(rd) = fs::read_dir(root.join("screenshots")) else {
            continue;
        };
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                continue;
            }
            let meta = entry.metadata().ok();
            shots.push(ScreenshotInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: game::path_str(&path),
                size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64),
                instance_id: instance_id.clone(),
                instance_name: instance_name.clone(),
            });
        }
    }
    shots.sort_by_key(|s| std::cmp::Reverse(s.modified));
    shots.truncate(300);
    shots
}

/// Zips a world folder and returns the archive path: worlds of an installation go to
/// `.minecraft/backups/instances/<id>/` (where «Бэкапы миров» finds them), shared ones to
/// `.minecraft/backups/`.
pub fn backup_world(path: &str) -> Result<String, String> {
    let world = PathBuf::from(path);
    let base = game::minecraft_dir();
    if !world.starts_with(&base) || !world.join("level.dat").is_file() {
        return Err("Это не папка мира Minecraft".into());
    }
    let instance_id = world
        .strip_prefix(base.join("instances"))
        .ok()
        .and_then(|rel| rel.components().next())
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    let dir = match instance_id {
        Some(id) => manual_dir(&id),
        None => base.join("backups"),
    };
    zip_world(&world, &dir).map(|target| game::path_str(&target))
}

/// How many automatic copies of each world are kept.
const AUTO_BACKUPS_KEPT: usize = 5;

/// Auto-backup before a launch: zips every world of the installation into
/// `.minecraft/backups/auto/<id>/` when `freq` says it is due (`launch` — always, `daily`,
/// `weekly`), keeps the newest 5 copies per world and stamps `lastBackup` in `instance.json`.
/// Returns how many worlds were saved.
pub fn auto_backup(instance_id: &str, freq: &str) -> Result<usize, String> {
    let instance = game::instance_dir(instance_id);
    let interval_ms: i64 = match freq {
        "daily" => 86_400_000,
        "weekly" => 7 * 86_400_000,
        _ => 0,
    };
    let last = game::read_json(&instance.join("instance.json"))
        .ok()
        .and_then(|m| m.get("lastBackup").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    if interval_ms > 0 && game::now_ms() - last < interval_ms {
        return Ok(0);
    }
    let Ok(rd) = fs::read_dir(instance.join("saves")) else {
        return Ok(0);
    };
    let worlds: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("level.dat").is_file())
        .collect();
    if worlds.is_empty() {
        return Ok(0);
    }
    let target = game::minecraft_dir().join("backups").join("auto").join(instance.file_name().unwrap_or_default());
    for world in &worlds {
        zip_world(world, &target)?;
        prune_backups(&target, &safe_name(world), AUTO_BACKUPS_KEPT);
    }
    game::update_instance_meta(instance_id, |meta| {
        meta.insert("lastBackup".into(), game::now_ms().into());
    });
    Ok(worlds.len())
}

/// Deletes all but the newest `keep` archives named `<prefix>_<timestamp>.zip`.
fn prune_backups(dir: &Path, prefix: &str, keep: usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut zips: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            name.ends_with(".zip") && name.strip_prefix(prefix).is_some_and(is_timestamp_zip)
        })
        .collect();
    // Timestamps sort lexically (YYYY-MM-DD_HH-MM-SS).
    zips.sort();
    let excess = zips.len().saturating_sub(keep);
    for old in zips.into_iter().take(excess) {
        let _ = fs::remove_file(old);
    }
}

/// `_2026-10-08_14-20-05.zip`
fn is_timestamp_zip(rest: &str) -> bool {
    rest.len() == "_2026-10-08_14-20-05.zip".len()
        && rest.starts_with('_')
        && rest[1..20].chars().all(|c| c.is_ascii_digit() || c == '-' || c == '_')
}

fn safe_name(world: &Path) -> String {
    world
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("world")
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Zips a world folder into `<dir>/<world>_<timestamp>.zip` (without `session.lock`).
fn zip_world(world: &Path, dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let folder = world.file_name().and_then(|n| n.to_str()).unwrap_or("world");
    let target = dir.join(format!("{}_{}.zip", safe_name(world), timestamp()));

    let file = File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut stack = vec![world.to_path_buf()];
    let mut buffer = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let rel = path
                .strip_prefix(world)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let name = format!("{folder}/{rel}");
            if path.is_dir() {
                zip.add_directory(format!("{name}/"), options).map_err(|e| e.to_string())?;
                stack.push(path);
            } else if rel != "session.lock" {
                buffer.clear();
                File::open(&path)
                    .and_then(|mut f| f.read_to_end(&mut buffer))
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                zip.start_file(name, options).map_err(|e| e.to_string())?;
                zip.write_all(&buffer).map_err(|e| e.to_string())?;
            }
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(target)
}

/// Copies a world into `.minecraft/instances/<id>/saves`, picking "Name (2)" if the folder exists.
/// Returns the new world folder.
/// `name`/`version` label a fresh installation (one that was never launched) in `instance.json`.
pub fn transfer_world(path: &str, instance_id: &str, name: Option<&str>, version: Option<&str>) -> Result<String, String> {
    let source = PathBuf::from(path);
    if !source.starts_with(game::minecraft_dir()) || !source.join("level.dat").is_file() {
        return Err("Это не папка мира Minecraft".into());
    }
    let saves = game::instance_dir(instance_id).join("saves");
    if source.starts_with(&saves) {
        return Err("Мир уже находится в этой установке".into());
    }
    fs::create_dir_all(&saves).map_err(|e| format!("{}: {e}", saves.display()))?;
    if let (Some(name), false) = (name, game::instance_dir(instance_id).join("instance.json").is_file()) {
        game::write_instance_meta(instance_id, name, version.unwrap_or_default());
    }
    let folder = source.file_name().and_then(|n| n.to_str()).unwrap_or("world").to_string();
    let mut target = saves.join(&folder);
    let mut n = 2;
    while target.exists() {
        target = saves.join(format!("{folder} ({n})"));
        n += 1;
    }
    copy_dir(&source, &target).map_err(|e| {
        let _ = fs::remove_dir_all(&target);
        format!("Не удалось скопировать мир: {e}")
    })?;
    Ok(game::path_str(&target))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&path, &dest)?;
        } else if entry.file_name() != "session.lock" {
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

/// `level.dat` → the `Data` compound.
fn read_level(path: &Path) -> Option<Nbt> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    GzDecoder::new(file).read_to_end(&mut bytes).ok()?;
    let root: Nbt = fastnbt::from_bytes(&bytes).ok()?;
    field(&root, "Data").cloned()
}

fn field<'a>(value: &'a Nbt, key: &str) -> Option<&'a Nbt> {
    match value {
        Nbt::Compound(map) => map.get(key),
        _ => None,
    }
}

fn as_string(value: &Nbt) -> Option<String> {
    match value {
        Nbt::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn as_int(value: &Nbt) -> Option<i64> {
    match value {
        Nbt::Byte(v) => Some(*v as i64),
        Nbt::Short(v) => Some(*v as i64),
        Nbt::Int(v) => Some(*v as i64),
        Nbt::Long(v) => Some(*v),
        _ => None,
    }
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&d) {
            for entry in rd.filter_map(|e| e.ok()) {
                match entry.metadata() {
                    Ok(m) if m.is_dir() => stack.push(entry.path()),
                    Ok(m) => total += m.len(),
                    Err(_) => {}
                }
            }
        }
    }
    total
}

/// Local-free UTC timestamp `YYYY-MM-DD_HH-MM-SS` for backup names.
fn timestamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}_{:02}-{:02}-{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------------------------
// Backups of an installation's worlds («Бэкапы миров»)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub file: String,
    pub path: String,
    /// World folder the archive was made from (`<folder>_<timestamp>.zip`).
    pub world: String,
    /// Creation time, ms since 1970 (from the name, which is in UTC).
    pub created: i64,
    pub size: u64,
    /// Made automatically before a launch (`backups/auto/<id>`).
    pub auto: bool,
    /// Folder of the installation's world this backup belongs to, if that world still exists.
    pub folder: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceBackups {
    pub worlds: Vec<WorldInfo>,
    pub backups: Vec<BackupEntry>,
    pub saves_dir: String,
    pub backups_dir: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub folder: String,
    /// Copy of the world as it was before the restore.
    pub safety: Option<String>,
}

/// Manual backups of an installation's worlds: `.minecraft/backups/instances/<id>/`.
fn manual_dir(instance_id: &str) -> PathBuf {
    game::minecraft_dir().join("backups").join("instances").join(instance_folder(instance_id))
}

/// Automatic ones (auto_backup): `.minecraft/backups/auto/<id>/`.
fn auto_dir(instance_id: &str) -> PathBuf {
    game::minecraft_dir().join("backups").join("auto").join(instance_folder(instance_id))
}

fn instance_folder(instance_id: &str) -> String {
    game::instance_dir(instance_id).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn instance_backups(instance_id: &str) -> InstanceBackups {
    let instance = game::instance_dir(instance_id);
    let name = game::read_json(&instance.join("instance.json"))
        .ok()
        .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .unwrap_or_else(|| instance_id.to_string());
    // So «Папка бэкапов» always has something to open.
    let _ = fs::create_dir_all(manual_dir(instance_id));
    let mut worlds = worlds_in(&instance, instance_id, &name);
    worlds.sort_by_key(|w| std::cmp::Reverse(w.last_played));
    let mut backups: Vec<BackupEntry> = [(manual_dir(instance_id), false), (auto_dir(instance_id), true)]
        .into_iter()
        .flat_map(|(dir, auto)| {
            fs::read_dir(&dir)
                .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| backup_entry(&e.path(), auto)).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();
    backups.sort_by_key(|b| std::cmp::Reverse(b.created));
    for b in &mut backups {
        b.folder = worlds.iter().find(|w| safe_name(Path::new(&w.folder)) == b.world).map(|w| w.folder.clone());
    }
    InstanceBackups {
        worlds,
        backups,
        saves_dir: game::path_str(&instance.join("saves")),
        backups_dir: game::path_str(&manual_dir(instance_id)),
    }
}

fn backup_entry(path: &Path, auto: bool) -> Option<BackupEntry> {
    let file = path.file_name()?.to_str()?.to_string();
    let stem = file.strip_suffix(".zip")?;
    // `<world>_YYYY-MM-DD_HH-MM-SS`
    let split = stem.len().checked_sub("_2026-10-08_14-20-05".len())?;
    let (world, rest) = stem.split_at(split);
    if !is_timestamp_zip(&format!("{rest}.zip")) {
        return None;
    }
    let meta = fs::metadata(path).ok()?;
    Some(BackupEntry {
        created: parse_timestamp(&rest[1..]).unwrap_or(0),
        world: world.to_string(),
        path: game::path_str(path),
        size: meta.len(),
        file,
        auto,
        folder: None,
    })
}

/// «Создать бэкап» for one world of the installation.
pub fn backup_instance_world(instance_id: &str, folder: &str) -> Result<BackupEntry, String> {
    let world = world_dir(instance_id, folder)?;
    let target = zip_world(&world, &manual_dir(instance_id))?;
    backup_entry(&target, false).ok_or_else(|| "Бэкап создан, но не читается".to_string())
}

/// Puts a world back from a backup of this installation. The archive is unpacked next to the
/// world first; the current world is zipped (safety copy) and swapped out only after that, so a
/// broken archive or a world still open in the game leaves everything as it was.
pub fn restore_backup(instance_id: &str, path: &str) -> Result<RestoreReport, String> {
    let zip_path = owned_backup(instance_id, path)?;
    let saves = game::instance_dir(instance_id).join("saves");
    fs::create_dir_all(&saves).map_err(|e| format!("{}: {e}", saves.display()))?;
    let stamp = timestamp();
    let staging = saves.join(format!(".signal-restore-{stamp}"));
    let folder = match unzip_world(&zip_path, &staging) {
        Ok(folder) => folder,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    let target = saves.join(&folder);
    let mut safety = None;
    let old = saves.join(format!(".signal-old-{stamp}"));
    if target.exists() {
        match zip_world(&target, &manual_dir(instance_id)) {
            Ok(copy) => safety = Some(game::path_str(&copy)),
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(format!("Не удалось сохранить текущий мир перед восстановлением: {e}"));
            }
        }
        if let Err(e) = fs::rename(&target, &old) {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("Мир «{folder}» сейчас занят (открыт в игре?): {e}"));
        }
    }
    if let Err(e) = fs::rename(&staging, &target) {
        if old.exists() {
            let _ = fs::rename(&old, &target);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("Не удалось восстановить мир: {e}"));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(RestoreReport { folder, safety })
}

pub fn delete_backup(instance_id: &str, path: &str) -> Result<(), String> {
    let zip_path = owned_backup(instance_id, path)?;
    fs::remove_file(&zip_path).map_err(|e| format!("Не удалось удалить бэкап: {e}"))
}

/// A `.zip` lying directly in this installation's backup folders — nothing else is touched.
fn owned_backup(instance_id: &str, path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let allowed = [manual_dir(instance_id), auto_dir(instance_id)];
    if !p.is_file() || !allowed.contains(&parent) || backup_entry(&p, false).is_none() {
        return Err("Это не бэкап этой установки".into());
    }
    Ok(p)
}

fn world_dir(instance_id: &str, folder: &str) -> Result<PathBuf, String> {
    if folder.is_empty() || folder.contains(['/', '\\']) || folder.contains("..") {
        return Err("Некорректное имя мира".into());
    }
    let world = game::instance_dir(instance_id).join("saves").join(folder);
    if !world.join("level.dat").is_file() {
        return Err(format!("Мир «{folder}» не найден"));
    }
    Ok(world)
}

/// Unpacks a backup into `dest` (the archive's single top folder is stripped) and returns that
/// folder's name. Entries must stay inside `dest`; the result must be a world (`level.dat`).
fn unzip_world(zip_path: &Path, dest: &Path) -> Result<String, String> {
    let file = File::open(zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Архив повреждён: {e}"))?;
    let mut top: Option<String> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| format!("Архив повреждён: {e}"))?;
        let rel = entry.enclosed_name().ok_or("В архиве недопустимый путь")?;
        let first = rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
        match &top {
            None => top = Some(first),
            Some(t) if *t != first => return Err("В архиве больше одного мира".into()),
            _ => {}
        }
    }
    let top = top.filter(|t| !t.is_empty() && !t.starts_with('.')).ok_or("Архив пустой")?;
    fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let Ok(inner) = rel.strip_prefix(&top) else { continue };
        if inner.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(inner);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let mut target = File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            std::io::copy(&mut entry, &mut target).map_err(|e| format!("{}: {e}", out.display()))?;
        }
    }
    if !dest.join("level.dat").is_file() {
        return Err("В архиве нет level.dat — это не мир Minecraft".into());
    }
    Ok(top)
}

/// `YYYY-MM-DD_HH-MM-SS` (UTC) → ms since 1970.
fn parse_timestamp(s: &str) -> Option<i64> {
    let n: Vec<i64> = s.split(['-', '_']).map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [y, mo, d, h, mi, se] = n.as_slice() else { return None };
    let days = days_from_civil(*y, *mo as u32, *d as u32);
    Some(((days * 86_400) + h * 3600 + mi * 60 + se) * 1000)
}

/// Inverse of `civil_from_days` (Howard Hinnant).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_auto_backups() {
        let dir = std::env::temp_dir().join(format!("signal-prune-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for day in 1..=7 {
            fs::write(dir.join(format!("World_2026-10-0{day}_10-00-00.zip")), b"").unwrap();
        }
        // Another world whose name starts the same must not be touched.
        fs::write(dir.join("World_2_2026-10-01_10-00-00.zip"), b"").unwrap();
        prune_backups(&dir, "World", 5);
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), 6);
        assert_eq!(left[0], "World_2026-10-03_10-00-00.zip");
        assert!(left.contains(&"World_2_2026-10-01_10-00-00.zip".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Reads the real `.minecraft`: `cargo test -- --ignored live_saves --nocapture`
    #[test]
    #[ignore]
    fn live_saves() {
        for w in list_worlds().iter().take(8) {
            println!("world {:?} [{}] mode {} diff {:?} ver {:?} last {:?} size {} icon {}", w.name, w.instance_name, w.game_type, w.difficulty, w.version, w.last_played, w.size, w.icon.is_some());
        }
        let shots = list_screenshots();
        println!("screenshots: {}", shots.len());
        for s in shots.iter().take(3) {
            println!("shot {} {} bytes", s.name, s.size);
        }
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn reads_nbt_fields() {
        let mut data = std::collections::HashMap::new();
        data.insert("LevelName".to_string(), Nbt::String("Тайга".into()));
        data.insert("GameType".to_string(), Nbt::Int(1));
        let root = Nbt::Compound(data);
        assert_eq!(field(&root, "LevelName").and_then(as_string).as_deref(), Some("Тайга"));
        assert_eq!(field(&root, "GameType").and_then(as_int), Some(1));
    }

    #[test]
    fn backup_names_and_restore_unpack() {
        assert_eq!(parse_timestamp("1970-01-02_00-00-01"), Some(86_401_000));
        assert_eq!(parse_timestamp("2026-10-10_12-30-00"), Some(1_791_635_400_000));
        let dir = std::env::temp_dir().join(format!("signal-restore-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let world = dir.join("saves").join("Мой мир");
        fs::create_dir_all(world.join("region")).unwrap();
        fs::write(world.join("level.dat"), b"nbt").unwrap();
        fs::write(world.join("region").join("r.0.0.mca"), b"chunks").unwrap();
        fs::write(world.join("session.lock"), b"x").unwrap();
        let zip = zip_world(&world, &dir.join("backups")).unwrap();
        let entry = backup_entry(&zip, false).unwrap();
        assert_eq!(entry.world, "Мой_мир");
        assert!(entry.created > 1_700_000_000_000);
        let out = dir.join("restored");
        assert_eq!(unzip_world(&zip, &out).unwrap(), "Мой мир");
        assert_eq!(fs::read(out.join("region").join("r.0.0.mca")).unwrap(), b"chunks");
        assert!(!out.join("session.lock").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Full cycle on a throw-away installation in the real game folder (removed afterwards):
    /// `cargo test --lib live_backup_restore -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_backup_restore() {
        let id = format!("signal-test-bk-{}", std::process::id());
        let inst = game::instance_dir(&id);
        let world = inst.join("saves").join("Тест");
        fs::create_dir_all(world.join("region")).unwrap();
        fs::write(world.join("level.dat"), b"v1").unwrap();
        let entry = backup_instance_world(&id, "Тест").unwrap();
        fs::write(world.join("level.dat"), b"v2").unwrap();
        fs::write(world.join("new.txt"), b"later").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100)); // the safety copy gets its own second
        let listed = instance_backups(&id);
        assert_eq!(listed.backups.len(), 1);
        assert_eq!(listed.backups[0].folder.as_deref(), Some("Тест"));
        let report = restore_backup(&id, &entry.path).unwrap();
        println!("{report:?}");
        assert_eq!(fs::read(world.join("level.dat")).unwrap(), b"v1");
        assert!(!world.join("new.txt").exists());
        assert!(report.safety.is_some());
        assert_eq!(instance_backups(&id).backups.len(), 2);
        assert!(restore_backup(&id, r"C:\Windows\win.ini").is_err());
        delete_backup(&id, &entry.path).unwrap();
        let _ = fs::remove_dir_all(&inst);
        let _ = fs::remove_dir_all(manual_dir(&id));
    }
}
