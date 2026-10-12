//! Computer memory for the RAM slider and «Очистить кэш».

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::game;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    pub total_mb: u64,
    pub available_mb: u64,
}

pub fn memory() -> MemoryInfo {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    MemoryInfo {
        total_mb: sys.total_memory() / 1_048_576,
        available_mb: sys.available_memory() / 1_048_576,
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheReport {
    pub files: usize,
    pub bytes: u64,
}

/// Removes what the launcher can always recreate: unfinished downloads (`*.part`) in the game
/// folders and extracted natives (`versions/<id>/natives`, unpacked again on the next launch).
/// Files in use by a running game are skipped.
pub fn clear_cache() -> CacheReport {
    let base = game::minecraft_dir();
    let mut report = CacheReport::default();
    for sub in ["versions", "libraries", "assets", "runtime", "instances"] {
        remove_parts(&base.join(sub), &mut report);
    }
    if let Ok(rd) = fs::read_dir(base.join("versions")) {
        for version in rd.filter_map(|e| e.ok()) {
            let natives = version.path().join("natives");
            if natives.is_dir() {
                remove_all(&natives, &mut report);
                let _ = fs::remove_dir_all(&natives);
            }
        }
    }
    report
}

fn remove_parts(dir: &Path, report: &mut CacheReport) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            remove_parts(&path, report);
        } else if path.extension().is_some_and(|ext| ext == "part") {
            remove_file(&path, report);
        }
    }
}

fn remove_all(dir: &Path, report: &mut CacheReport) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            remove_all(&path, report);
            let _ = fs::remove_dir(&path);
        } else {
            remove_file(&path, report);
        }
    }
}

fn remove_file(path: &Path, report: &mut CacheReport) {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if fs::remove_file(path).is_ok() {
        report.files += 1;
        report.bytes += size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_part_files() {
        let dir = std::env::temp_dir().join(format!("signal-cache-test-{}", std::process::id()));
        let nested = dir.join("a/b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("lib.jar.part"), b"12345").unwrap();
        fs::write(nested.join("lib.jar"), b"keep").unwrap();
        let mut report = CacheReport::default();
        remove_parts(&dir, &mut report);
        assert_eq!((report.files, report.bytes), (1, 5));
        assert!(nested.join("lib.jar").is_file());
        assert!(!nested.join("lib.jar.part").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_memory() {
        let m = memory();
        assert!(m.total_mb > 0 && m.available_mb <= m.total_mb);
    }
}
