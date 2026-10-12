//! Forge / NeoForge auto-install: picks the build for the Minecraft version, downloads the
//! official installer and runs it headless (`--installClient <.minecraft>`) with a Java that
//! fits the game, turning its console output into `download-progress` events.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::game::{self, Loader, Progress};

const FORGE_PROMOS: &str = "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
const NEOFORGE_VERSIONS: &str = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

/// Launch progress share of the loader install (the game download continues from here).
const START: f64 = 1.0;
const END: f64 = 17.0;

/// Installs Forge or NeoForge for Minecraft `mc` into `game_dir`; returns the version id
/// (`1.20.1-forge-47.4.10`, `neoforge-21.1.256`). `wanted` pins the build (modpacks).
pub async fn install(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    loader: Loader,
    mc: &str,
    wanted: Option<&str>,
) -> Result<String, String> {
    let name = if loader == Loader::NeoForge { "NeoForge" } else { "Forge" };
    progress.step("loader", format!("Поиск сборки {name} для {mc}…"), START);
    let (build, url) = match loader {
        Loader::NeoForge => {
            let v = match wanted {
                Some(v) => v.to_string(),
                None => neoforge_build(client, mc).await?,
            };
            let url = format!("{NEOFORGE_MAVEN}/{v}/neoforge-{v}-installer.jar");
            (v, url)
        }
        _ => {
            let v = match wanted {
                Some(v) => v.to_string(),
                None => forge_build(client, mc).await?,
            };
            let url = format!("{FORGE_MAVEN}/{mc}-{v}/forge-{mc}-{v}-installer.jar");
            (v, url)
        }
    };

    // The installer's processors run on the Java the game itself needs (17 for 1.20.1, 21 for 1.20.5+).
    // Nested steps report on their own 0–100 scale; keep them inside the loader's share.
    progress.set_span(Some((START, START + 1.0)));
    let vanilla_json = game::ensure_vanilla_json(client, progress, game_dir, mc).await;
    progress.set_span(None);
    vanilla_json?;
    let vanilla = game::read_json(&game::version_json(game_dir, mc))?;
    let component = vanilla.pointer("/javaVersion/component").and_then(Value::as_str).unwrap_or("jre-legacy").to_string();
    let required = vanilla.pointer("/javaVersion/majorVersion").and_then(Value::as_u64).unwrap_or(8) as u32;
    progress.step("loader", format!("Подготовка Java {required} для установщика {name}…"), START + 1.0);
    progress.set_span(Some((START + 1.0, START + 2.0)));
    let java = game::ensure_java(client, progress, game_dir, &component, required).await;
    progress.set_span(None);
    let java = game::console_java(&java?);
    match game::java_major(&java) {
        Some(major) if game::java_fits(major, required) => {}
        Some(major) => {
            return Err(format!("Установщику {name} {build} нужна Java {required}, а подготовлена Java {major} ({}).", java.display()))
        }
        None => return Err(format!("Не удалось запустить Java для установщика {name}: {}", java.display())),
    }

    let dir = game_dir.join("signal-installers");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let jar = dir.join(url.rsplit('/').next().unwrap_or("installer.jar"));
    progress.step("loader", format!("Скачивание установщика {name} {build}…"), START + 2.0);
    game::download(client, &url, &jar, None).await?;

    let profile = read_install_profile(&jar).map_err(|e| {
        format!("{name} для Minecraft {mc} использует старый установщик, его Signal пока не поддерживает (Forge — с 1.12.2, NeoForge — с 1.20.2). {e}")
    })?;
    let version_id = profile
        .get("version")
        .and_then(Value::as_str)
        .ok_or("В установщике нет install_profile.json с id версии")?
        .to_string();
    // The installer walks the libraries of install_profile.json and of the bundled version.json.
    let total_libraries = profile.get("libraries").and_then(Value::as_array).map_or(0, Vec::len)
        + read_jar_json(&jar, profile.get("json").and_then(Value::as_str).unwrap_or("/version.json"))
            .ok()
            .and_then(|v| v.get("libraries").and_then(Value::as_array).map(Vec::len))
            .unwrap_or(0);
    let total_processors = profile
        .get("processors")
        .and_then(Value::as_array)
        .map_or(0, |list| list.iter().filter(|p| runs_on_client(p)).count());

    // The installer refuses to install a client without the official launcher's profile file.
    let profiles = game_dir.join("launcher_profiles.json");
    if !profiles.is_file() {
        fs::write(&profiles, r#"{"profiles":{},"settings":{},"version":3}"#).map_err(|e| format!("{}: {e}", profiles.display()))?;
    }

    progress.step("loader", format!("Запуск установщика {name} {build}…"), START + 4.0);
    let log_path = dir.join(format!("{version_id}-install.log"));
    run_installer(&java, &jar, game_dir, &log_path, progress, name, total_libraries, total_processors)?;

    progress.step("loader", format!("Финализация профиля {name}…"), END);
    if !game::version_json(game_dir, &version_id).is_file() {
        return Err(format!(
            "Установщик {name} завершился, но профиль {version_id} не появился. Журнал: {}",
            log_path.display()
        ));
    }
    println!("[signal] {name} {build} installed as {version_id}");
    Ok(version_id)
}

/// Forge: the recommended build for this Minecraft version, else the latest one.
async fn forge_build(client: &reqwest::Client, mc: &str) -> Result<String, String> {
    let promos = game::get_json(client, FORGE_PROMOS).await?;
    let pick = |kind: &str| promos.pointer(&format!("/promos/{mc}-{kind}")).and_then(Value::as_str).map(str::to_string);
    pick("recommended")
        .or_else(|| pick("latest"))
        .ok_or_else(|| format!("Для Minecraft {mc} нет сборки Forge"))
}

/// NeoForge: newest stable build for this Minecraft version (a beta if there is no stable one).
async fn neoforge_build(client: &reqwest::Client, mc: &str) -> Result<String, String> {
    let data = game::get_json(client, NEOFORGE_VERSIONS).await?;
    let versions: Vec<&str> = data
        .get("versions")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    pick_neoforge(&versions, mc).ok_or_else(|| format!("Для Minecraft {mc} нет сборки NeoForge"))
}

/// NeoForge numbers builds after the game: 1.21.1 → `21.1.x`, 1.21 → `21.0.x`,
/// 26.3 → `26.3.0.x`, 26.1.2 → `26.1.2.x`. (1.20.1 had NeoForge under Forge's coordinates
/// and is not offered here.)
pub(crate) fn neoforge_prefix(mc: &str) -> Option<String> {
    let parts: Vec<&str> = mc.split('.').collect();
    if parts.first() == Some(&"1") {
        let minor = parts.get(1)?;
        let patch = parts.get(2).copied().unwrap_or("0");
        if minor.parse::<u32>().ok()? < 20 || (*minor == "20" && patch == "1") {
            return None;
        }
        Some(format!("{minor}.{patch}."))
    } else {
        let patch = parts.get(2).copied().unwrap_or("0");
        Some(format!("{}.{}.{patch}.", parts.first()?, parts.get(1)?))
    }
}

fn pick_neoforge(versions: &[&str], mc: &str) -> Option<String> {
    let prefix = neoforge_prefix(mc)?;
    let key = |v: &&str| game::version_key(v);
    let matching: Vec<&str> = versions.iter().copied().filter(|v| v.starts_with(&prefix)).collect();
    let stable = matching.iter().copied().filter(|v| !v.contains('-')).max_by_key(key);
    stable.or_else(|| matching.iter().copied().max_by_key(key)).map(str::to_string)
}

fn read_install_profile(jar: &Path) -> Result<Value, String> {
    read_jar_json(jar, "install_profile.json")
        .map_err(|e| format!("Это не установщик Forge/NeoForge нового формата ({e})"))
}

fn read_jar_json(jar: &Path, name: &str) -> Result<Value, String> {
    let file = File::open(jar).map_err(|e| format!("{}: {e}", jar.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("повреждён {}: {e}", jar.display()))?;
    let mut entry = archive.by_name(name.trim_start_matches('/')).map_err(|_| format!("нет {name}"))?;
    let mut text = String::new();
    entry.read_to_string(&mut text).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))
}

fn runs_on_client(processor: &Value) -> bool {
    match processor.get("sides").and_then(Value::as_array) {
        Some(sides) => sides.iter().any(|s| s.as_str() == Some("client")),
        None => true,
    }
}

/// Runs the installer without a window and maps its console output to progress:
/// «Considering library» lines → libraries, `====` separators → processors.
#[allow(clippy::too_many_arguments)]
fn run_installer(
    java: &Path,
    jar: &Path,
    game_dir: &Path,
    log_path: &Path,
    progress: &Progress,
    name: &str,
    total_libraries: usize,
    total_processors: usize,
) -> Result<(), String> {
    let mut command = Command::new(java);
    command
        .arg("-jar")
        .arg(jar)
        .arg("--installClient")
        .arg(game_dir)
        .current_dir(jar.parent().unwrap_or(game_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    game::set_creation_flags(&mut command, game::Priority::Normal);
    let mut child = command.spawn().map_err(|e| format!("Не удалось запустить установщик {name}: {e}"))?;

    // stderr goes into the same log from its own thread so neither pipe can fill up and stall.
    let log = File::create(log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
    let err_log = log.try_clone().map_err(|e| e.to_string())?;
    let stderr = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        let mut out = err_log;
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = writeln!(out, "{line}");
            }
        }
    });

    let mut out = log;
    let mut libraries = 0usize;
    let mut processors = 0usize;
    let mut in_processors = false;
    let mut tail: Vec<String> = Vec::new();
    let span_libraries = (START + 5.0, START + 9.0);
    let span_processors = (START + 9.0, END - 1.0);
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = writeln!(out, "{line}");
            let trimmed = line.trim();
            if trimmed.starts_with("Considering library") || trimmed.starts_with("Downloading library") {
                if trimmed.starts_with("Considering") {
                    libraries += 1;
                }
                let share = if total_libraries > 0 { (libraries as f64 / total_libraries as f64).min(1.0) } else { 0.5 };
                progress.report(
                    "loader",
                    format!("Скачивание библиотек {name} ({libraries}/{})…", total_libraries.max(libraries)),
                    span_libraries.0 + (span_libraries.1 - span_libraries.0) * share,
                    libraries,
                    total_libraries,
                    false,
                );
            } else if trimmed == "Building Processors" {
                in_processors = true;
            } else if in_processors && trimmed.starts_with("=====") {
                processors += 1;
                let total = total_processors.max(processors);
                progress.report(
                    "loader",
                    format!("Запуск процессоров {name} ({processors}/{total})…"),
                    span_processors.0 + (span_processors.1 - span_processors.0) * processors as f64 / total as f64,
                    processors,
                    total,
                    true,
                );
            }
            if !trimmed.is_empty() {
                tail.push(trimmed.to_string());
                if tail.len() > 6 {
                    tail.remove(0);
                }
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = err_thread.join();
    if !status.success() {
        let why = tail.iter().rev().find(|l| l.contains("rror") || l.contains("xception") || l.contains("ailed")).or(tail.last());
        return Err(format!(
            "Установщик {name} завершился с ошибкой (код {}){}. Журнал: {}",
            status.code().map_or("?".into(), |c| c.to_string()),
            why.map(|w| format!(": {w}")).unwrap_or_default(),
            log_path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neoforge_numbering() {
        assert_eq!(neoforge_prefix("1.21.1").as_deref(), Some("21.1."));
        assert_eq!(neoforge_prefix("1.21").as_deref(), Some("21.0."));
        assert_eq!(neoforge_prefix("1.20.4").as_deref(), Some("20.4."));
        assert_eq!(neoforge_prefix("26.3").as_deref(), Some("26.3.0."));
        assert_eq!(neoforge_prefix("26.1.2").as_deref(), Some("26.1.2."));
        assert_eq!(neoforge_prefix("1.20.1"), None);
        assert_eq!(neoforge_prefix("1.12.2"), None);
    }

    #[test]
    fn picks_newest_stable_neoforge() {
        let list = ["21.1.9", "21.1.99", "21.1.100", "21.1.101-beta", "21.0.5", "26.3.0.56-beta", "26.3.0.57-beta"];
        assert_eq!(pick_neoforge(&list, "1.21.1").as_deref(), Some("21.1.100"));
        assert_eq!(pick_neoforge(&list, "26.3").as_deref(), Some("26.3.0.57-beta"));
        assert_eq!(pick_neoforge(&list, "1.20.4"), None);
    }

    #[test]
    fn client_processors_only() {
        let both: Value = serde_json::json!({ "jar": "x" });
        let server: Value = serde_json::json!({ "sides": ["server"] });
        let client: Value = serde_json::json!({ "sides": ["client", "server"] });
        assert!(runs_on_client(&both) && runs_on_client(&client) && !runs_on_client(&server));
    }

    /// Real NeoForge 1.21.1 install into a temp folder (downloads ~300 MB):
    /// `cargo test -- --ignored live_neoforge_install --nocapture`
    #[test]
    #[ignore]
    fn live_neoforge_install() {
        let dir = std::env::temp_dir().join("signal-neoforge-live");
        fs::create_dir_all(&dir).unwrap();
        let result = tauri::async_runtime::block_on(async {
            let client = game::http_client()?;
            install(&client, &Progress::console(), &dir, Loader::NeoForge, "1.21.1", None).await
        });
        println!("result: {result:?} in {}", dir.display());
        let id = result.unwrap();
        assert!(game::version_json(&dir, &id).is_file());
    }
}
