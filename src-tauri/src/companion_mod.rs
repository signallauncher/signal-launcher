//! Signal Companion mod delivery: before a Fabric game starts, the jar built for its Minecraft version goes
//! into the installation's `mods/` folder.
//!
//! The server publishes `mod/manifest.json` (scripts/build-mod-dist.py): every jar with its sha256, and which jar
//! each game version loads (the symbol check in mod/tools/symbols.py decides that, snapshots included). Jars are
//! cached once per content hash under `<game dir>/signal-companion/jars/`, checked against the manifest's
//! sha256 and copied as `mods/signal-companion.jar`. Without network the last manifest is used; a version with
//! no compatible jar gets none (an older copy is removed so it cannot crash the game). A player who disabled the
//! mod in the mods list (`signal-companion.jar.disabled`) is left alone. Nothing here ever blocks the launch.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

const SERVER: &str = "https://signallauncher.h4ck.me";
const MANIFEST: &str = "/mod/manifest.json";
pub const JAR_NAME: &str = "signal-companion.jar";

/// What happened, for the launch message: the target id of the installed jar, or `None` when the version has none.
pub async fn install(client: &reqwest::Client, instance: &Path, game_version: &str) -> Result<Option<String>, String> {
    let mods = instance.join("mods");
    if mods.join(format!("{JAR_NAME}.disabled")).exists() {
        return Ok(None);
    }
    let manifest = manifest(client).await?;
    let Some(target) = manifest.pointer(&format!("/games/{}", escape(game_version))).and_then(Value::as_str) else {
        remove_ours(&mods, None);
        return Ok(None);
    };
    let jar = manifest
        .pointer(&format!("/jars/{target}"))
        .ok_or_else(|| format!("в манифесте нет сборки {target}"))?;
    let url = jar.get("url").and_then(Value::as_str).ok_or("в манифесте нет ссылки на jar")?;
    let sha = jar.get("sha256").and_then(Value::as_str).ok_or("в манифесте нет sha256")?.to_ascii_lowercase();
    let size = jar.get("size").and_then(Value::as_u64);

    let cached = cache_dir().join("jars").join(format!("{sha}.jar"));
    if !matches(&cached, &sha) {
        let _ = fs::remove_file(&cached);
        crate::game::download(client, &absolute(url), &cached, size).await?;
        if !matches(&cached, &sha) {
            let _ = fs::remove_file(&cached);
            return Err("скачанный файл не совпал с контрольной суммой".into());
        }
    }
    fs::create_dir_all(&mods).map_err(|e| format!("{}: {e}", mods.display()))?;
    let dest = mods.join(JAR_NAME);
    if !matches(&dest, &sha) {
        fs::copy(&cached, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    remove_ours(&mods, Some(&dest));
    Ok(Some(target.to_string()))
}

/// The manifest from the server (saved for offline starts), else the saved one.
async fn manifest(client: &reqwest::Client) -> Result<Value, String> {
    let saved = cache_dir().join("manifest.json");
    match crate::game::get_json(client, &absolute(MANIFEST)).await {
        Ok(fresh) if fresh.get("games").is_some() => {
            let _ = fs::create_dir_all(cache_dir());
            let _ = fs::write(&saved, fresh.to_string());
            Ok(fresh)
        }
        Ok(_) => Err("сервер вернул неполный манифест мода".into()),
        Err(e) => crate::game::read_json(&saved).map_err(|_| e),
    }
}

fn cache_dir() -> PathBuf {
    crate::game::minecraft_dir().join("signal-companion")
}

fn absolute(url: &str) -> String {
    if url.starts_with("http") { url.to_string() } else { format!("{SERVER}{url}") }
}

/// JSON pointer escaping for version ids ("1.14 Pre-Release 1", "23w13a_or_b").
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn matches(path: &Path, sha: &str) -> bool {
    fs::read(path).map(|bytes| hex(&Sha256::digest(&bytes)) == sha).unwrap_or(false)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Other copies of the mod in `mods/` (older names, manual downloads): two would load twice.
fn remove_ours(mods: &Path, keep: Option<&Path>) {
    let Ok(entries) = fs::read_dir(mods) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let ours = (name.starts_with("signal-companion") || name.starts_with("signal_companion")) && name.ends_with(".jar");
        if ours && Some(path.as_path()) != keep {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_other_copies_only() {
        let dir = std::env::temp_dir().join(format!("sc-mod-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in ["signal-companion.jar", "signal_companion-0.1.0+g21.jar", "sodium.jar", "signal-companion.jar.disabled"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        remove_ours(&dir, Some(&dir.join("signal-companion.jar")));
        let mut left: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["signal-companion.jar", "signal-companion.jar.disabled", "sodium.jar"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Against the real server: `cargo test --lib live_install -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_install() {
        let instance = std::env::temp_dir().join(format!("sc-mod-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&instance);
        fs::create_dir_all(instance.join("mods")).unwrap();
        fs::write(instance.join("mods").join("signal_companion-0.0.1+old.jar"), b"stale").unwrap();
        let client = crate::game::http_client().unwrap();
        for (mc, expected) in [("1.21.1", Some("g21")), ("1.14.4", Some("g14")), ("26.3", Some("g263")), ("26.4-snapshot-4", None)] {
            let got = tauri::async_runtime::block_on(install(&client, &instance, mc)).unwrap();
            let jar = instance.join("mods").join(JAR_NAME);
            println!("{mc}: {got:?}, {} bytes", fs::metadata(&jar).map(|m| m.len()).unwrap_or(0));
            assert_eq!(got.as_deref(), expected);
            assert_eq!(jar.is_file(), expected.is_some(), "{mc}");
        }
        assert!(!instance.join("mods").join("signal_companion-0.0.1+old.jar").exists());
        let _ = fs::remove_dir_all(&instance);
    }

    #[test]
    fn pointer_keys_are_escaped() {
        let m: Value = serde_json::json!({"games": {"1.14 Pre-Release 1": "g14", "a/b": "x"}});
        assert_eq!(m.pointer(&format!("/games/{}", escape("1.14 Pre-Release 1"))).unwrap(), "g14");
        assert_eq!(m.pointer(&format!("/games/{}", escape("a/b"))).unwrap(), "x");
    }
}
