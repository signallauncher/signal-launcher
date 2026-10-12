//! Game install + launch, the way standalone launchers do it:
//! 1. version manifest → version JSON → client.jar from Mojang (piston-meta / piston-data),
//! 2. libraries (and LWJGL 2 natives for old versions, extracted next to the version),
//! 3. asset index + asset objects (resources.download.minecraft.net),
//! 4. the Java runtime the version asks for (Mojang's java-runtime builds),
//!
//! then builds the classpath and arguments and starts `javaw`.
//! Fabric is installed through meta.fabricmc.net. Progress goes to the UI as `download-progress`.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const RESOURCES_URL: &str = "https://resources.download.minecraft.net";
const LIBRARIES_URL: &str = "https://libraries.minecraft.net/";
const JAVA_RUNTIMES: &str =
    "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions/loader";
const PARALLEL_DOWNLOADS: usize = 16;

const LAUNCHER_NAME: &str = "SignalLauncher";
const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(windows)]
const CP_SEPARATOR: &str = ";";
#[cfg(not(windows))]
const CP_SEPARATOR: &str = ":";

#[cfg(windows)]
const JAVA_BIN: &str = "javaw.exe";
#[cfg(not(windows))]
const JAVA_BIN: &str = "java";

#[cfg(target_os = "windows")]
const OS_NAME: &str = "windows";
#[cfg(target_os = "macos")]
const OS_NAME: &str = "osx";
#[cfg(all(unix, not(target_os = "macos")))]
const OS_NAME: &str = "linux";

#[cfg(target_os = "windows")]
const RUNTIME_PLATFORM: &str = "windows-x64";
#[cfg(target_os = "macos")]
const RUNTIME_PLATFORM: &str = "mac-os";
#[cfg(all(unix, not(target_os = "macos")))]
const RUNTIME_PLATFORM: &str = "linux";

/// Result of «ИГРАТЬ», serialized to the frontend as `{ mode, title, message, ... }`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOutcome {
    pub mode: String,
    pub title: String,
    pub message: String,
    pub version_id: Option<String>,
    pub java: Option<String>,
    pub pid: Option<u32>,
    /// The chosen Java did not fit the version and Mojang's was used instead.
    pub java_note: Option<String>,
}

pub struct LaunchOptions {
    pub version_label: String,
    /// Installation id from the UI; the game runs with `--gameDir .minecraft/instances/<id>`.
    pub instance_id: String,
    pub instance_name: String,
    pub username: String,
    pub ram_gb: u32,
    pub extra_jvm_args: Vec<String>,
    pub priority: Priority,
    /// Minimize the launcher while the game runs and bring it back when the game exits.
    pub hide_launcher: bool,
    /// Auto-backup of this installation's worlds before the start: `launch`, `daily` or `weekly`.
    pub backup: Option<String>,
    /// Microsoft account session; `None` starts the game offline with `username`.
    pub auth: Option<crate::auth::Session>,
    /// Discord Rich Presence while the game runs.
    pub discord: bool,
    /// `host[:port]` to join right after the game starts.
    pub server: Option<String>,
    /// Offline account: the launcher's skin as a resource pack (skinpack.rs). `None` removes it.
    pub skin: Option<crate::skinpack::SkinPayload>,
    /// A Java picked in the settings (global or the installation's own); `None` — Mojang's
    /// runtime for the version. One that does not fit the version is replaced (see `java_note`).
    pub java_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Priority {
    Normal,
    High,
    Realtime,
}

impl Priority {
    pub fn parse(value: &str) -> Self {
        match value {
            "high" => Priority::High,
            "realtime" => Priority::Realtime,
            _ => Priority::Normal,
        }
    }

    /// Windows process creation flags (`HIGH_PRIORITY_CLASS`, `REALTIME_PRIORITY_CLASS`).
    /// Without admin rights Windows quietly turns «realtime» into «high».
    #[cfg_attr(not(windows), allow(dead_code))]
    fn creation_flag(self) -> u32 {
        match self {
            Priority::Normal => 0,
            Priority::High => 0x0000_0080,
            Priority::Realtime => 0x0000_0100,
        }
    }
}

/// Sent to the UI as `game-exit` when the game process ends.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameExit {
    instance_id: String,
    instance_name: String,
    code: Option<i32>,
    crashed: bool,
    seconds: u64,
    log: String,
    crash_report: Option<String>,
    /// Likely cause when the game crashed (see `console::diagnose`).
    reason: Option<crate::console::CrashReason>,
}

/// Merged version profile (child profile + everything it inherits from).
struct Profile {
    id: String,
    jar_id: String,
    main_class: String,
    version_type: String,
    asset_index: String,
    asset_index_url: Option<String>,
    asset_index_size: Option<u64>,
    client_url: Option<String>,
    client_size: Option<u64>,
    java_component: Option<String>,
    /// `javaVersion.majorVersion` (8, 17, 21…).
    java_major: Option<u32>,
    libraries: Vec<Value>,
    game_args: Vec<Value>,
    jvm_args: Vec<Value>,
    legacy_args: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Progress events
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    stage: String,
    label: String,
    percent: u8,
    current: usize,
    total: usize,
}

/// Where progress goes. The Tauri sink is built in `Progress::new`, so code that only reports
/// progress (installers, downloads) does not pull the window runtime into test binaries.
type ProgressSink = Box<dyn Fn(ProgressEvent) + Send + Sync>;

pub(crate) struct Progress {
    sink: ProgressSink,
    last_emit: Mutex<Instant>,
    /// While set, 0–100 % of a nested step maps into this range of the overall bar.
    span: Mutex<Option<(f64, f64)>>,
}

impl Progress {
    pub(crate) fn new(app: AppHandle, event: &'static str) -> Self {
        Self::with_sink(Box::new(move |e| {
            let _ = app.emit(event, e);
        }))
    }

    /// Progress without a window: lines go to stdout (live tests).
    #[cfg(test)]
    pub(crate) fn console() -> Self {
        Self::with_sink(Box::new(|e| println!("[{:>3}%] {}", e.percent, e.label)))
    }

    fn with_sink(sink: ProgressSink) -> Self {
        Self {
            sink,
            last_emit: Mutex::new(Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now)),
            span: Mutex::new(None),
        }
    }

    /// Squeezes the percentages of a nested step (e.g. Java download inside the Forge install)
    /// into `from..to`; `None` restores the normal scale.
    pub(crate) fn set_span(&self, span: Option<(f64, f64)>) {
        *self.span.lock().unwrap_or_else(|e| e.into_inner()) = span;
    }

    /// Sends a progress event to the UI. Frequent updates are throttled to ~8 per second.
    pub(crate) fn report(&self, stage: &str, label: impl Into<String>, percent: f64, current: usize, total: usize, force: bool) {
        if !force {
            let mut last = self.last_emit.lock().unwrap_or_else(|e| e.into_inner());
            if last.elapsed() < Duration::from_millis(120) {
                return;
            }
            *last = Instant::now();
        }
        let percent = match *self.span.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((from, to)) => from + (to - from) * percent.clamp(0.0, 100.0) / 100.0,
            None => percent,
        };
        (self.sink)(ProgressEvent {
            stage: stage.into(),
            label: label.into(),
            percent: percent.clamp(0.0, 100.0).round() as u8,
            current,
            total,
        });
    }

    pub(crate) fn step(&self, stage: &str, label: impl Into<String>, percent: f64) {
        self.report(stage, label, percent, 0, 0, true);
    }
}

// ---------------------------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------------------------

/// The game folder: `%APPDATA%\.minecraft` unless moved in «Сменить папку…» (gamedir.rs).
pub fn minecraft_dir() -> PathBuf {
    crate::gamedir::current()
}

/// `.minecraft/instances/<id>` — the installation's own game directory.
pub fn instance_dir(id: &str) -> PathBuf {
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    let safe = if safe.is_empty() { "default".to_string() } else { safe };
    minecraft_dir().join("instances").join(safe)
}

/// Writes `instance.json` so other screens (worlds, screenshots) can show a readable name.
/// Other fields (last launch, play time, last backup) are kept.
pub(crate) fn write_instance_meta(id: &str, name: &str, version: &str) {
    update_instance_meta(id, |meta| {
        meta.insert("id".into(), id.into());
        meta.insert("name".into(), name.into());
        meta.insert("version".into(), version.into());
    });
}

/// Read-modify-write of `instance.json`; creates the installation folder and stamps `created`.
pub(crate) fn update_instance_meta(id: &str, edit: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let dir = instance_dir(id);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("instance.json");
    let mut meta = read_json(&path).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default();
    meta.entry("created").or_insert_with(|| now_ms().into());
    edit(&mut meta);
    let _ = fs::write(path, Value::Object(meta).to_string());
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub async fn launch(app: AppHandle, opts: LaunchOptions) -> Result<LaunchOutcome, String> {
    let label = opts.version_label.trim().to_string();
    if label.is_empty() {
        return Err("Не выбрана версия игры".into());
    }
    let progress = Progress::new(app.clone(), "download-progress");
    let game_dir = minecraft_dir();
    fs::create_dir_all(&game_dir).map_err(|e| format!("Не удалось создать {}: {e}", game_dir.display()))?;
    // Versions, libraries, assets and Java are shared; mods, configs, worlds live per installation.
    let instance = instance_dir(&opts.instance_id);
    fs::create_dir_all(&instance).map_err(|e| format!("Не удалось создать {}: {e}", instance.display()))?;
    write_instance_meta(&opts.instance_id, &opts.instance_name, &label);
    let client = http_client()?;
    println!("[signal] launch «{label}» in {}", instance.display());

    // A failed backup never blocks the game; the result goes into the launch message.
    let mut backup_note = String::new();
    if let Some(freq) = opts.backup.clone() {
        progress.step("backup", "Бэкап миров перед запуском…", 0.0);
        let id = opts.instance_id.clone();
        backup_note = match tauri::async_runtime::spawn_blocking(move || crate::saves::auto_backup(&id, &freq)).await {
            Ok(Ok(0)) => String::new(),
            Ok(Ok(n)) => format!(" Бэкап миров: {n}."),
            Ok(Err(e)) => format!(" Бэкап не удался: {e}."),
            Err(e) => format!(" Бэкап не удался: {e}."),
        };
    }

    progress.step("prepare", format!("Подготовка версии {label}…"), 0.0);
    // Imported modpacks pin an exact loader build (instance.json → loaderVersion).
    let loader_version = read_json(&instance.join("instance.json"))
        .ok()
        .and_then(|m| m.get("loaderVersion").and_then(Value::as_str).map(str::to_string));
    let version_id = prepare_version(&client, &progress, &game_dir, &label, loader_version.as_deref()).await?;
    ensure_parents(&client, &progress, &game_dir, &version_id).await?;
    let profile = load_profile(&game_dir, &version_id, 0)?;
    println!("[signal] profile {} (jar {}, java {:?})", profile.id, profile.jar_id, profile.java_component);

    // client.jar
    let jar = game_dir
        .join("versions")
        .join(&profile.jar_id)
        .join(format!("{}.jar", profile.jar_id));
    if !is_ready(&jar, profile.client_size) {
        let url = profile
            .client_url
            .clone()
            .ok_or_else(|| format!("В профиле {} нет ссылки на client.jar", profile.jar_id))?;
        progress.step("client", format!("Загрузка client.jar ({})…", profile.jar_id), 6.0);
        download(&client, &url, &jar, profile.client_size).await?;
    }
    progress.step("client", "client.jar готов", 18.0);

    // The launcher's skin in an offline game (skinpack.rs); a failure never blocks the launch.
    let mut skin_note = String::new();
    match (&opts.auth, &opts.skin) {
        (None, Some(skin)) => {
            let mc = parse_label(&label).1.unwrap_or_else(|| profile.jar_id.clone());
            if let Err(e) = crate::skinpack::apply(&instance, &jar, &mc, skin) {
                skin_note = format!(" Скин в игре не применён: {e}.");
            }
        }
        _ => crate::skinpack::remove(&instance),
    }

    // libraries + natives
    let (mut classpath, native_jars) = ensure_libraries(&client, &progress, &game_dir, &profile).await?;
    classpath.push(jar);
    let natives_dir = game_dir.join("versions").join(&profile.id).join("natives");
    if !native_jars.is_empty() {
        progress.step("natives", "Распаковка нативных библиотек…", 40.0);
    }
    extract_natives(&native_jars, &natives_dir)?;

    // assets
    let game_assets = ensure_assets(&client, &progress, &game_dir, &instance, &profile).await?;

    // java: the chosen one if it fits this version, else Mojang's runtime for it
    let component = profile.java_component.clone().unwrap_or_else(|| "jre-legacy".into());
    let required = profile.java_major.unwrap_or(8);
    let mut java_note = None;
    let chosen = opts.java_path.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(PathBuf::from);
    let java = match chosen {
        Some(path) => match (path.is_file(), java_major(&path)) {
            (true, Some(major)) if java_fits(major, required) => path,
            (true, Some(major)) => {
                java_note = Some(format!(
                    "Выбранная Java {major} не подходит для {} — нужна Java {required}. Запущена Java {required} от Mojang.",
                    profile.id
                ));
                ensure_java(&client, &progress, &game_dir, &component, required).await?
            }
            _ => {
                java_note = Some(format!("Выбранная Java не запускается ({}). Запущена Java {required} от Mojang.", path.display()));
                ensure_java(&client, &progress, &game_dir, &component, required).await?
            }
        },
        None => ensure_java(&client, &progress, &game_dir, &component, required).await?,
    };
    println!("[signal] java: {}", java.display());

    progress.step("launch", "Запуск игры…", 99.0);
    // Licensed session (msa) or offline name with the standard offline UUID.
    let (username, uuid, token, user_type) = match &opts.auth {
        Some(session) => (session.name.clone(), session.uuid.clone(), session.token.clone(), "msa"),
        None => {
            let name = sanitize_username(&opts.username);
            let uuid = offline_uuid(&name);
            (name, uuid, "0".to_string(), "legacy")
        }
    };
    let libraries_dir = game_dir.join("libraries");
    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", username.clone());
    vars.insert("version_name", profile.id.clone());
    vars.insert("game_directory", path_str(&instance));
    vars.insert("assets_root", path_str(&game_dir.join("assets")));
    vars.insert("game_assets", path_str(&game_assets));
    vars.insert("assets_index_name", profile.asset_index.clone());
    vars.insert("auth_uuid", uuid.clone());
    vars.insert("auth_access_token", token.clone());
    // Pre-1.7 versions read the session as one string.
    vars.insert("auth_session", if opts.auth.is_some() { format!("token:{token}:{uuid}") } else { "0".into() });
    vars.insert("auth_xuid", "0".into());
    vars.insert("clientid", LAUNCHER_NAME.into());
    vars.insert("user_type", user_type.into());
    vars.insert("user_properties", "{}".into());
    vars.insert("version_type", profile.version_type.clone());
    vars.insert("natives_directory", path_str(&natives_dir));
    vars.insert("launcher_name", LAUNCHER_NAME.into());
    vars.insert("launcher_version", LAUNCHER_VERSION.into());
    vars.insert(
        "classpath",
        classpath.iter().map(|p| path_str(p)).collect::<Vec<_>>().join(CP_SEPARATOR),
    );
    vars.insert("classpath_separator", CP_SEPARATOR.into());
    vars.insert("library_directory", path_str(&libraries_dir));

    let mut args: Vec<String> = vec![format!("-Xmx{}G", opts.ram_gb.clamp(1, 64)), "-Xms512M".into()];
    args.extend(opts.extra_jvm_args.iter().cloned());
    if profile.jvm_args.is_empty() {
        args.push(substitute("-Djava.library.path=${natives_directory}", &vars));
        args.push("-cp".into());
        args.push(vars["classpath"].clone());
    } else {
        push_args(&mut args, &profile.jvm_args, &vars);
    }
    args.push(profile.main_class.clone());
    if let Some(legacy) = &profile.legacy_args {
        args.extend(legacy.split_whitespace().map(|a| substitute(a, &vars)));
    } else {
        push_args(&mut args, &profile.game_args, &vars);
    }
    // Join a server on start: 1.20+ has Quick Play, older versions take --server/--port.
    if let Some(server) = opts.server.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let quick_play = serde_json::to_string(&profile.game_args).unwrap_or_default().contains("quickPlayMultiplayer");
        if quick_play {
            args.extend(["--quickPlayMultiplayer".into(), server.to_string()]);
        } else {
            let (host, port) = match server.rsplit_once(':') {
                Some((h, p)) if p.parse::<u16>().is_ok() && !h.contains(':') => (h.to_string(), p.to_string()),
                _ => (server.to_string(), "25565".to_string()),
            };
            args.extend(["--server".into(), host, "--port".into(), port]);
        }
    }

    // The whole session goes to signal-launch.log (Minecraft keeps writing its own logs/latest.log);
    // the console gets the same lines live (console.rs).
    let log_path = instance.join("signal-launch.log");
    let mut command = Command::new(&java);
    command
        .args(&args)
        .current_dir(&instance)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    set_creation_flags(&mut command, opts.priority);

    let mut child = command
        .spawn()
        .map_err(|e| format!("Не удалось запустить Java ({}): {e}", java.display()))?;
    let mut streams: Vec<(Box<dyn std::io::Read + Send>, bool)> = Vec::new();
    if let Some(out) = child.stdout.take() {
        streams.push((Box::new(out), false));
    }
    if let Some(err) = child.stderr.take() {
        streams.push((Box::new(err), true));
    }
    let console_app = app.clone();
    let attached = crate::console::attach(
        &app.state::<crate::console::Console>(),
        &opts.instance_id,
        streams,
        File::create(&log_path).ok(),
        move |batch| {
            let _ = console_app.emit("game-log", batch);
        },
    );
    let pid = child.id();
    crate::gamedir::RUNNING_GAMES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    println!("[signal] game started, pid {pid}, priority {:?}", opts.priority);
    update_instance_meta(&opts.instance_id, |meta| {
        meta.insert("lastPlayed".into(), now_ms().into());
    });
    progress.step("done", "Игра запущена", 100.0);

    if opts.hide_launcher {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.minimize();
        }
    }
    if opts.discord {
        crate::discord::set_playing(opts.instance_name.clone(), profile.id.clone(), now_ms());
    }
    watch_game(app, child, attached, &opts, path_str(&log_path), opts.hide_launcher);

    Ok(LaunchOutcome {
        mode: "started".into(),
        title: "Игра запускается!".into(),
        message: format!(
            "{} · {} {username}. Журнал: {}.{backup_note}{skin_note}",
            profile.id,
            if opts.auth.is_some() { "лицензия," } else { "офлайн," },
            log_path.display()
        ),
        version_id: Some(profile.id),
        java: Some(path_str(&java)),
        pid: Some(pid),
        java_note,
    })
}

/// Waits for the game on a separate thread: adds the session to `playTime`, restores the
/// launcher window and tells the UI (`game-exit`), including the newest crash report if the
/// game crashed.
fn watch_game(
    app: AppHandle,
    mut child: std::process::Child,
    console: crate::console::Attached,
    opts: &LaunchOptions,
    log: String,
    restore: bool,
) {
    let instance_id = opts.instance_id.clone();
    let instance_name = opts.instance_name.clone();
    let discord = opts.discord;
    let started = Instant::now();
    let started_at = std::time::SystemTime::now();
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|status| status.code());
        crate::gamedir::RUNNING_GAMES.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        // Everything the game printed is read and sent before the exit is announced.
        let session = console.finish();
        let seconds = started.elapsed().as_secs();
        let crashed = code != Some(0);
        update_instance_meta(&instance_id, |meta| {
            let total = meta.get("playTime").and_then(Value::as_u64).unwrap_or(0) + seconds;
            meta.insert("playTime".into(), total.into());
            meta.insert("lastExitCode".into(), code.map(Value::from).unwrap_or(Value::Null));
        });
        let crash_report = if crashed { newest_crash_report(&instance_dir(&instance_id), started_at) } else { None };
        let reason = crashed.then(|| {
            let report = crash_report.as_ref().and_then(|p| fs::read_to_string(p).ok());
            crate::console::diagnose(&session.snapshot(), report.as_deref())
        });
        println!("[signal] game exited: code {code:?} after {seconds}s");
        if discord {
            crate::discord::clear();
        }
        if restore {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        let _ = app.emit(
            "game-exit",
            GameExit { instance_id, instance_name, code, crashed, seconds, log, crash_report, reason },
        );
    });
}

/// `crash-reports/crash-*.txt` (or a JVM `hs_err_pid*.log`) written during this session, if any.
fn newest_crash_report(instance: &Path, since: std::time::SystemTime) -> Option<String> {
    let reports = fs::read_dir(instance.join("crash-reports")).into_iter().flatten();
    let jvm = fs::read_dir(instance)
        .into_iter()
        .flatten()
        .filter(|e| e.as_ref().is_ok_and(|e| e.file_name().to_string_lossy().starts_with("hs_err_pid")));
    reports
        .chain(jvm)
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(modified, _)| *modified >= since)
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path_str(&path))
}

#[cfg(windows)]
pub(crate) fn set_creation_flags(command: &mut Command, priority: Priority) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW | priority.creation_flag());
}

#[cfg(not(windows))]
pub(crate) fn set_creation_flags(_command: &mut Command, _priority: Priority) {}

// ---------------------------------------------------------------------------------------------
// Versions: vanilla from Mojang, Fabric from meta.fabricmc.net, Forge/NeoForge if installed
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub(crate) enum Loader {
    Vanilla,
    Fabric,
    Forge,
    NeoForge,
}

/// "Release 1.21.9" → (Vanilla, "1.21.9"), "Fabric 1.21.1" → (Fabric, "1.21.1"), …
pub(crate) fn parse_label(label: &str) -> (Loader, Option<String>) {
    let lower = label.to_lowercase();
    // The version id is the last word: "Release 26.3", "Snapshot 26.4-snapshot-3", "Beta b1.7.3".
    let number = label
        .split_whitespace()
        .last()
        .filter(|t| t.chars().any(|c| c.is_ascii_digit()))
        .map(str::to_string);
    let loader = if lower.starts_with("fabric") {
        Loader::Fabric
    } else if lower.starts_with("neoforge") {
        Loader::NeoForge
    } else if lower.starts_with("forge") {
        Loader::Forge
    } else {
        Loader::Vanilla
    };
    (loader, number)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub id: String,
    /// `release`, `snapshot`, `old_beta`, `old_alpha`
    pub kind: String,
    pub release_time: String,
    pub installed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionList {
    pub latest_release: String,
    pub latest_snapshot: String,
    pub versions: Vec<VersionEntry>,
    /// true when the list came from the local cache because Mojang was unreachable.
    pub offline: bool,
}

/// All game versions from Mojang's manifest (newest first), cached for offline use.
pub async fn list_versions() -> Result<VersionList, String> {
    let game_dir = minecraft_dir();
    let cache = game_dir.join("signal_version_manifest.json");
    let client = http_client()?;
    let (manifest, offline) = match get_json(&client, VERSION_MANIFEST).await {
        Ok(json) => {
            let _ = fs::create_dir_all(&game_dir);
            let _ = fs::write(&cache, json.to_string());
            (json, false)
        }
        Err(e) => (
            read_json(&cache).map_err(|_| format!("Нет связи с Mojang и нет сохранённого списка версий ({e})"))?,
            true,
        ),
    };
    let latest = |key: &str| {
        manifest
            .pointer(&format!("/latest/{key}"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let versions = manifest
        .get("versions")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| {
                    let id = v.get("id")?.as_str()?.to_string();
                    Some(VersionEntry {
                        installed: version_json(&game_dir, &id).is_file(),
                        kind: v.get("type").and_then(Value::as_str).unwrap_or("release").to_string(),
                        release_time: v.get("releaseTime").and_then(Value::as_str).unwrap_or_default().to_string(),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(VersionList {
        latest_release: latest("release"),
        latest_snapshot: latest("snapshot"),
        versions,
        offline,
    })
}

/// Makes sure the version JSON exists locally and returns its id.
async fn prepare_version(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    label: &str,
    loader_version: Option<&str>,
) -> Result<String, String> {
    if version_json(game_dir, label).is_file() {
        return Ok(label.to_string());
    }
    let (loader, number) = parse_label(label);
    let mc = number.ok_or_else(|| format!("Не удалось определить версию игры из «{label}»"))?;
    match loader {
        Loader::Vanilla => {
            ensure_vanilla_json(client, progress, game_dir, &mc).await?;
            Ok(mc)
        }
        Loader::Fabric => {
            let installed = match loader_version {
                Some(lv) => Some(format!("fabric-loader-{lv}-{mc}")).filter(|id| version_json(game_dir, id).is_file()),
                None => resolve_installed(game_dir, label),
            };
            match installed {
                Some(id) => Ok(id),
                None => install_fabric(client, progress, game_dir, &mc, loader_version).await,
            }
        }
        Loader::Forge | Loader::NeoForge => {
            let installed = match loader_version {
                Some(lv) => Some(if loader == Loader::NeoForge { format!("neoforge-{lv}") } else { format!("{mc}-forge-{lv}") })
                    .filter(|id| version_json(game_dir, id).is_file()),
                None => resolve_installed(game_dir, label),
            };
            match installed {
                Some(id) => Ok(id),
                None => crate::loaders::install(client, progress, game_dir, loader, &mc, loader_version).await,
            }
        }
    }
}

pub(crate) fn version_json(game_dir: &Path, id: &str) -> PathBuf {
    game_dir.join("versions").join(id).join(format!("{id}.json"))
}

/// Downloads `versions/<id>/<id>.json` from Mojang's version manifest if it is missing.
pub(crate) async fn ensure_vanilla_json(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    id: &str,
) -> Result<(), String> {
    let path = version_json(game_dir, id);
    if path.is_file() {
        return Ok(());
    }
    progress.step("manifest", "Загрузка манифеста версий…", 1.0);
    let manifest = get_json(client, VERSION_MANIFEST).await?;
    let entry = manifest
        .get("versions")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find(|v| v.get("id").and_then(Value::as_str) == Some(id)))
        .ok_or_else(|| format!("Версия {id} не найдена на серверах Mojang"))?;
    let url = entry
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("В манифесте нет ссылки на профиль {id}"))?;
    progress.step("version", format!("Загрузка профиля версии {id}…"), 3.0);
    download(client, url, &path, None).await
}

/// Installs the latest stable Fabric Loader for `mc` and returns the new version id.
/// Fabric Loader `wanted` (a modpack's pin), else the newest stable build for `mc`.
async fn install_fabric(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    mc: &str,
    wanted: Option<&str>,
) -> Result<String, String> {
    ensure_vanilla_json(client, progress, game_dir, mc).await?;
    progress.step("fabric", format!("Установка Fabric Loader для {mc}…"), 4.0);
    let chosen = match wanted {
        Some(v) => v.to_string(),
        None => {
            let loaders = get_json(client, &format!("{FABRIC_META}/{mc}")).await?;
            let list = loaders.as_array().cloned().unwrap_or_default();
            list.iter()
                .find(|l| l.pointer("/loader/stable").and_then(Value::as_bool) == Some(true))
                .or_else(|| list.first())
                .and_then(|l| l.pointer("/loader/version"))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("Fabric пока не поддерживает Minecraft {mc}"))?
                .to_string()
        }
    };
    let id = format!("fabric-loader-{chosen}-{mc}");
    let url = format!("{FABRIC_META}/{mc}/{chosen}/profile/json");
    download(client, &url, &version_json(game_dir, &id), None).await?;
    Ok(id)
}

/// Downloads the vanilla parents of a modded profile (inheritsFrom) when they are missing.
async fn ensure_parents(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    id: &str,
) -> Result<(), String> {
    let mut current = id.to_string();
    for _ in 0..5 {
        let json = read_json(&version_json(game_dir, &current))?;
        let Some(parent) = json.get("inheritsFrom").and_then(Value::as_str).map(str::to_string) else {
            return Ok(());
        };
        ensure_vanilla_json(client, progress, game_dir, &parent).await?;
        current = parent;
    }
    Ok(())
}

/// Finds an already installed version for a UI label ("Forge 1.20.1" → "1.20.1-forge-47.3.0", …).
/// Build of the loader an installation runs on, from its installed version id
/// (`fabric-loader-0.16.9-1.21.1` → `0.16.9`, `1.21.1-forge-52.1.0` → `52.1.0`, `neoforge-21.1.256`).
pub(crate) fn installed_loader_version(game_dir: &Path, label: &str) -> Option<String> {
    let id = resolve_installed(game_dir, label)?;
    let (loader, mc) = parse_label(label);
    let mc = mc?;
    match loader {
        Loader::Fabric => id.strip_prefix("fabric-loader-")?.strip_suffix(&format!("-{mc}")).map(str::to_string),
        Loader::Forge => id.strip_prefix(&format!("{mc}-forge-")).map(str::to_string),
        Loader::NeoForge => id.strip_prefix("neoforge-").map(str::to_string),
        Loader::Vanilla => None,
    }
}

fn resolve_installed(game_dir: &Path, label: &str) -> Option<String> {
    let versions = game_dir.join("versions");
    let mut ids: Vec<String> = fs::read_dir(&versions)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|id| version_json(game_dir, id).is_file())
        .collect();
    if ids.iter().any(|id| id == label) {
        return Some(label.to_string());
    }
    ids.sort_by(|a, b| b.cmp(a)); // newest loader build first
    let (loader, number) = parse_label(label);
    let number = number?;
    let found = match loader {
        Loader::Fabric => ids
            .iter()
            .find(|id| id.starts_with("fabric-loader-") && id.ends_with(&format!("-{number}"))),
        // Same numbering the installer uses (1.21.1 → neoforge-21.1.x, 26.3 → neoforge-26.3.0.x);
        // the newest build by number, not by string («21.1.256» > «21.1.99»).
        Loader::NeoForge => {
            let prefix = format!("neoforge-{}", crate::loaders::neoforge_prefix(&number)?);
            ids.iter().filter(|id| id.starts_with(&prefix)).max_by_key(|id| version_key(id))
        }
        // `1.21.1-forge-52.1.0`; the dash keeps 1.21.1 from matching 1.21.10.
        Loader::Forge => {
            let prefix = format!("{number}-forge");
            ids.iter().filter(|id| id.to_lowercase().starts_with(&prefix)).max_by_key(|id| version_key(id))
        }
        Loader::Vanilla => ids.iter().find(|id| **id == number),
    };
    found.cloned()
}

fn load_profile(game_dir: &Path, id: &str, depth: u8) -> Result<Profile, String> {
    if depth > 5 {
        return Err("Слишком глубокая цепочка inheritsFrom в профиле версии".into());
    }
    let json = read_json(&version_json(game_dir, id))?;
    let str_of = |key: &str| json.get(key).and_then(Value::as_str).map(str::to_string);
    let arr_of = |ptr: &str| json.pointer(ptr).and_then(Value::as_array).cloned().unwrap_or_default();

    let own_jar = game_dir.join("versions").join(id).join(format!("{id}.jar"));
    let mut profile = match str_of("inheritsFrom") {
        Some(parent_id) => {
            let mut parent = load_profile(game_dir, &parent_id, depth + 1)?;
            parent.id = id.to_string();
            if own_jar.is_file() {
                parent.jar_id = id.to_string();
            }
            parent
        }
        None => Profile {
            id: id.to_string(),
            jar_id: id.to_string(),
            main_class: String::new(),
            version_type: "release".into(),
            asset_index: "legacy".into(),
            asset_index_url: None,
            asset_index_size: None,
            client_url: None,
            client_size: None,
            java_component: None,
            java_major: None,
            libraries: Vec::new(),
            game_args: Vec::new(),
            jvm_args: Vec::new(),
            legacy_args: None,
        },
    };

    // Child libraries go first so they win over the parent's during deduplication.
    let mut libraries = arr_of("/libraries");
    libraries.append(&mut profile.libraries);
    profile.libraries = libraries;
    profile.game_args.extend(arr_of("/arguments/game"));
    profile.jvm_args.extend(arr_of("/arguments/jvm"));
    if let Some(main) = str_of("mainClass") {
        profile.main_class = main;
    }
    if let Some(kind) = str_of("type") {
        profile.version_type = kind;
    }
    if let Some(jar) = str_of("jar") {
        profile.jar_id = jar;
    }
    if let Some(legacy) = str_of("minecraftArguments") {
        profile.legacy_args = Some(legacy);
    }
    if let Some(index) = json
        .pointer("/assetIndex/id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| str_of("assets"))
    {
        profile.asset_index = index;
    }
    if let Some(url) = json.pointer("/assetIndex/url").and_then(Value::as_str) {
        profile.asset_index_url = Some(url.to_string());
        profile.asset_index_size = json.pointer("/assetIndex/size").and_then(Value::as_u64);
    }
    if let Some(url) = json.pointer("/downloads/client/url").and_then(Value::as_str) {
        profile.client_url = Some(url.to_string());
        profile.client_size = json.pointer("/downloads/client/size").and_then(Value::as_u64);
    }
    if let Some(component) = json.pointer("/javaVersion/component").and_then(Value::as_str) {
        profile.java_component = Some(component.to_string());
    }
    if let Some(major) = json.pointer("/javaVersion/majorVersion").and_then(Value::as_u64) {
        profile.java_major = Some(major as u32);
    }
    if depth == 0 && profile.main_class.is_empty() {
        return Err(format!("В профиле {id} не указан mainClass"));
    }
    Ok(profile)
}

// ---------------------------------------------------------------------------------------------
// Libraries, natives, assets, Java
// ---------------------------------------------------------------------------------------------

/// Downloads every library the profile needs; returns (classpath jars, native jars to extract).
async fn ensure_libraries(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    profile: &Profile,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), String> {
    let libs_dir = game_dir.join("libraries");
    let arch = if cfg!(target_pointer_width = "64") { "64" } else { "32" };
    let mut classpath = Vec::new();
    let mut natives = Vec::new();
    let mut jobs = Vec::new();
    let mut seen = HashSet::new();
    let mut missing_local = 0usize;

    for lib in &profile.libraries {
        if !rules_allow(lib.get("rules")) {
            continue;
        }
        let name = lib.get("name").and_then(Value::as_str).unwrap_or("");

        // LWJGL 2 era: natives live in a classifier jar that gets extracted.
        if let Some(classifier) = lib.pointer(&format!("/natives/{OS_NAME}")).and_then(Value::as_str) {
            let classifier = classifier.replace("${arch}", arch);
            if let Some(d) = lib.pointer(&format!("/downloads/classifiers/{classifier}")) {
                if let (Some(url), Some(path)) = (d.get("url").and_then(Value::as_str), d.get("path").and_then(Value::as_str)) {
                    let dest = libs_dir.join(path);
                    jobs.push(Job { url: url.to_string(), dest: dest.clone(), size: d.get("size").and_then(Value::as_u64) });
                    natives.push(dest);
                }
            }
        }

        let (dest, url, size) = if let Some(artifact) = lib.pointer("/downloads/artifact") {
            let Some(path) = artifact.get("path").and_then(Value::as_str) else {
                continue;
            };
            (
                libs_dir.join(path),
                artifact.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
                artifact.get("size").and_then(Value::as_u64),
            )
        } else if lib.get("natives").is_some() && lib.get("downloads").is_some() {
            continue; // natives-only entry, handled above
        } else {
            // Fabric / old Forge style: maven coordinates + repository url.
            let Some(rel) = maven_path(name) else {
                continue;
            };
            let base = lib.get("url").and_then(Value::as_str).unwrap_or(LIBRARIES_URL);
            let base = if base.ends_with('/') { base.to_string() } else { format!("{base}/") };
            (libs_dir.join(&rel), format!("{base}{rel}"), lib.get("size").and_then(Value::as_u64))
        };

        if !name.is_empty() && !seen.insert(library_key(name)) {
            continue;
        }
        if url.is_empty() {
            // Generated locally by an installer (Forge/NeoForge); nothing to download.
            if !dest.is_file() {
                missing_local += 1;
            }
        } else {
            jobs.push(Job { url, dest: dest.clone(), size });
        }
        classpath.push(dest);
    }

    if missing_local > 0 {
        return Err(format!(
            "Не хватает {missing_local} библиотек, которые создаёт установщик Forge/NeoForge. Переустановите загрузчик."
        ));
    }
    download_many(client, progress, dedupe(jobs), "libraries", "Скачивание библиотек", 18.0, 40.0).await?;
    Ok((classpath, natives))
}

fn extract_natives(jars: &[PathBuf], dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for jar in jars {
        let file = File::open(jar).map_err(|e| format!("{}: {e}", jar.display()))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", jar.display()))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().to_string();
            if entry.is_dir() || name.starts_with("META-INF/") || name.contains("..") {
                continue;
            }
            let out = dir.join(&name);
            if out.is_file() {
                continue;
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut target = File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            io::copy(&mut entry, &mut target).map_err(|e| format!("{}: {e}", out.display()))?;
        }
    }
    Ok(())
}

/// Downloads the asset index and all asset objects; returns the `${game_assets}` folder.
async fn ensure_assets(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    instance: &Path,
    profile: &Profile,
) -> Result<PathBuf, String> {
    let assets = game_dir.join("assets");
    let index_path = assets.join("indexes").join(format!("{}.json", profile.asset_index));
    if let Some(url) = &profile.asset_index_url {
        if !is_ready(&index_path, profile.asset_index_size) {
            progress.step("assets", "Загрузка индекса ассетов…", 41.0);
            download(client, url, &index_path, profile.asset_index_size).await?;
        }
    }
    if !index_path.is_file() {
        return Ok(assets);
    }
    let index = read_json(&index_path)?;

    let mut jobs = Vec::new();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    if let Some(objects) = index.get("objects").and_then(Value::as_object) {
        for (name, object) in objects {
            let Some(hash) = object.get("hash").and_then(Value::as_str) else {
                continue;
            };
            if hash.len() < 2 {
                continue;
            }
            let dest = assets.join("objects").join(&hash[..2]).join(hash);
            entries.push((name.clone(), dest.clone()));
            if seen.insert(hash.to_string()) {
                jobs.push(Job {
                    url: format!("{RESOURCES_URL}/{}/{hash}", &hash[..2]),
                    dest,
                    size: object.get("size").and_then(Value::as_u64),
                });
            }
        }
    }
    download_many(client, progress, jobs, "assets", "Скачивание ассетов", 42.0, 85.0).await?;

    // Very old versions read assets by file name instead of by hash.
    let is_virtual = index.get("virtual").and_then(Value::as_bool).unwrap_or(false);
    let to_resources = index.get("map_to_resources").and_then(Value::as_bool).unwrap_or(false);
    if is_virtual || to_resources {
        let target = if to_resources {
            instance.join("resources")
        } else {
            assets.join("virtual").join(&profile.asset_index)
        };
        for (name, source) in &entries {
            let dest = target.join(name);
            if !dest.is_file() {
                if let Some(parent) = dest.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::copy(source, &dest);
            }
        }
        if is_virtual {
            return Ok(target);
        }
    }
    Ok(assets)
}

/// The Java runtime the version asks for: already downloaded, freshly downloaded from Mojang,
/// or — if that fails — a Java installed on the system.
/// Mojang's runtime for `component`; if it cannot be downloaded, a system Java whose major
/// version fits `required` (see `java_fits`).
pub(crate) async fn ensure_java(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    component: &str,
    required: u32,
) -> Result<PathBuf, String> {
    let java = game_dir
        .join("runtime")
        .join(component)
        .join(RUNTIME_PLATFORM)
        .join(component)
        .join("bin")
        .join(JAVA_BIN);
    if java.is_file() {
        return Ok(java);
    }
    match download_runtime(client, progress, game_dir, component).await {
        Ok(java) => Ok(java),
        Err(e) => {
            println!("[signal] runtime download failed: {e}");
            find_system_java(required)
                .map_err(|why| format!("Не удалось скачать Java {required} от Mojang ({e}). {why}"))
        }
    }
}

async fn download_runtime(
    client: &reqwest::Client,
    progress: &Progress,
    game_dir: &Path,
    component: &str,
) -> Result<PathBuf, String> {
    progress.step("java", format!("Загрузка Java ({component})…"), 85.0);
    let all = get_json(client, JAVA_RUNTIMES).await?;
    let manifest_url = all
        .get(RUNTIME_PLATFORM)
        .and_then(|platform| platform.get(component))
        .and_then(Value::as_array)
        .and_then(|builds| builds.first())
        .and_then(|build| build.pointer("/manifest/url"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Mojang не публикует Java «{component}» для этой системы"))?
        .to_string();
    let manifest = get_json(client, &manifest_url).await?;
    let root = game_dir.join("runtime").join(component).join(RUNTIME_PLATFORM).join(component);
    let files = manifest
        .get("files")
        .and_then(Value::as_object)
        .ok_or_else(|| "пустой манифест Java".to_string())?;

    let mut jobs = Vec::new();
    let mut executables = Vec::new();
    for (path, info) in files {
        let dest = root.join(path);
        match info.get("type").and_then(Value::as_str) {
            Some("directory") => {
                fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            }
            Some("file") => {
                if let Some(raw) = info.pointer("/downloads/raw") {
                    if let Some(url) = raw.get("url").and_then(Value::as_str) {
                        if info.get("executable").and_then(Value::as_bool) == Some(true) {
                            executables.push(dest.clone());
                        }
                        jobs.push(Job { url: url.to_string(), dest, size: raw.get("size").and_then(Value::as_u64) });
                    }
                }
            }
            _ => {}
        }
    }
    download_many(client, progress, jobs, "java", "Скачивание Java", 85.0, 98.0).await?;
    for path in &executables {
        make_executable(path);
    }
    let java = root.join("bin").join(JAVA_BIN);
    if java.is_file() {
        Ok(java)
    } else {
        Err(format!("в скачанной Java нет {}", java.display()))
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// Every Java the system has: JAVA_HOME, the usual vendor folders, PATH.
fn system_java_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(home) = std::env::var("JAVA_HOME") {
        out.push(PathBuf::from(home).join("bin").join(JAVA_BIN));
    }
    if cfg!(windows) {
        for vendor in [
            r"C:\Program Files\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft",
            r"C:\Program Files\Zulu",
            r"C:\Program Files\BellSoft",
        ] {
            let mut jdks = subdirs(Path::new(vendor));
            jdks.sort_by(|a, b| b.cmp(a));
            out.extend(jdks.into_iter().map(|jdk| jdk.join("bin").join(JAVA_BIN)));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        out.extend(std::env::split_paths(&paths).map(|dir| dir.join(JAVA_BIN)));
    }
    out.retain(|java| java.is_file());
    out.dedup();
    out
}

/// A Java the launcher can offer in «Путь к Java».
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaInfo {
    pub path: String,
    pub major: u32,
    /// `mojang` (downloaded for a version) or `system`.
    pub source: String,
}

/// Mojang runtimes already downloaded into `.minecraft/runtime` plus every system Java.
pub fn list_java() -> Vec<JavaInfo> {
    let runtime = minecraft_dir().join("runtime");
    let mut found: Vec<(PathBuf, &str)> = subdirs(&runtime)
        .into_iter()
        .filter_map(|component| {
            let name = component.file_name()?.to_os_string();
            let java = component.join(RUNTIME_PLATFORM).join(name).join("bin").join(JAVA_BIN);
            java.is_file().then_some((java, "mojang"))
        })
        .collect();
    found.extend(system_java_candidates().into_iter().map(|j| (j, "system")));
    let mut seen = HashSet::new();
    let mut out: Vec<JavaInfo> = found
        .into_iter()
        .filter(|(p, _)| seen.insert(p.to_string_lossy().to_lowercase()))
        .filter_map(|(path, source)| Some(JavaInfo { major: java_major(&path)?, path: path_str(&path), source: source.into() }))
        .collect();
    out.sort_by(|a, b| b.major.cmp(&a.major).then(a.source.cmp(&b.source)));
    out
}

/// «Указать путь вручную…»: the file must be a working Java (`java.exe` → `javaw.exe` next to it,
/// so the game opens no console window).
pub fn check_java(path: &str) -> Result<JavaInfo, String> {
    let mut java = PathBuf::from(path.trim());
    if java.is_dir() {
        java = java.join("bin").join(JAVA_BIN);
    }
    let name = java.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_lowercase();
    if !["java.exe", "javaw.exe", "java"].contains(&name.as_str()) {
        return Err("Выберите файл java.exe или javaw.exe (папка bin в каталоге Java)".into());
    }
    let windowed = java.with_file_name(JAVA_BIN);
    if windowed.is_file() {
        java = windowed;
    }
    let major = java_major(&java).ok_or_else(|| format!("Эта Java не запускается: {}", java.display()))?;
    Ok(JavaInfo { path: path_str(&java), major, source: "system".into() })
}

/// The first system Java that fits `required`, or why there is none.
fn find_system_java(required: u32) -> Result<PathBuf, String> {
    let mut seen = Vec::new();
    for java in system_java_candidates() {
        match java_major(&java) {
            Some(major) if java_fits(major, required) => return Ok(java),
            Some(major) => seen.push(major),
            None => {}
        }
    }
    seen.sort_unstable();
    seen.dedup();
    if seen.is_empty() {
        Err(format!("Java {required} в системе не найдена."))
    } else {
        let found = seen.iter().map(|m| format!("Java {m}")).collect::<Vec<_>>().join(", ");
        Err(format!("Нужна Java {required}, а в системе есть только {found}."))
    }
}

/// Java 8 games (1.16 and older, old Forge) need exactly 8; newer ones run on that version or later.
pub(crate) fn java_fits(major: u32, required: u32) -> bool {
    if required <= 8 {
        major == 8
    } else {
        major >= required
    }
}

/// Major version of a Java binary from `java -version`: 8 for "1.8.0_382", 21 for "21.0.7".
pub(crate) fn java_major(java: &Path) -> Option<u32> {
    let mut command = Command::new(console_java(java));
    command.arg("-version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    set_creation_flags(&mut command, Priority::Normal);
    let output = command.output().ok()?;
    parse_java_major(&String::from_utf8_lossy(&output.stderr))
}

/// `java.exe` next to `javaw.exe` (its output can be read); the binary itself otherwise.
pub(crate) fn console_java(java: &Path) -> PathBuf {
    let console = java.with_file_name(if cfg!(windows) { "java.exe" } else { "java" });
    if console.is_file() {
        console
    } else {
        java.to_path_buf()
    }
}

fn parse_java_major(text: &str) -> Option<u32> {
    let start = text.find('"')? + 1;
    let rest = &text[start..];
    let version = &rest[..rest.find('"')?];
    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------------------------

struct Job {
    url: String,
    dest: PathBuf,
    size: Option<u64>,
}

fn dedupe(jobs: Vec<Job>) -> Vec<Job> {
    let mut seen = HashSet::new();
    jobs.into_iter().filter(|job| seen.insert(job.dest.clone())).collect()
}

pub(crate) fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("SignalLauncher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())
}

/// A file counts as downloaded when it exists and (if known) has the expected size.
pub(crate) fn is_ready(path: &Path, size: Option<u64>) -> bool {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => size.map_or(meta.len() > 0, |s| meta.len() == s),
        _ => false,
    }
}

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client.get(url).send().await.map_err(|e| format!("сеть: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response.bytes().await.map(|b| b.to_vec()).map_err(|e| format!("сеть: {e}"))
}

pub(crate) async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let bytes = fetch_bytes(client, url).await.map_err(|e| format!("{url}: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{url}: {e}"))
}

pub(crate) fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("Не удалось прочитать {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("Повреждён файл {}: {e}", path.display()))
}

/// Downloads one file (3 attempts), writing through a `.part` file so a broken download never
/// looks complete.
pub(crate) async fn download(client: &reqwest::Client, url: &str, dest: &Path, size: Option<u64>) -> Result<(), String> {
    if is_ready(dest, size) {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);

    let mut last_error = String::new();
    for _attempt in 0..3 {
        match fetch_bytes(client, url).await {
            Ok(bytes) => {
                if let Some(expected) = size {
                    if bytes.len() as u64 != expected {
                        last_error = format!("получено {} байт вместо {expected}", bytes.len());
                        continue;
                    }
                }
                fs::write(&part, &bytes).map_err(|e| format!("{}: {e}", part.display()))?;
                fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                return Ok(());
            }
            Err(e) => last_error = e,
        }
    }
    Err(format!("Не удалось скачать {url}: {last_error}"))
}

/// Downloads many files in parallel and reports «label (done/total)…» between `from`..`to` percent.
async fn download_many(
    client: &reqwest::Client,
    progress: &Progress,
    jobs: Vec<Job>,
    stage: &str,
    label: &str,
    from: f64,
    to: f64,
) -> Result<(), String> {
    let jobs: Vec<Job> = jobs.into_iter().filter(|job| !is_ready(&job.dest, job.size)).collect();
    let total = jobs.len();
    if total == 0 {
        return Ok(());
    }
    progress.report(stage, format!("{label} (0/{total})…"), from, 0, total, true);
    let mut results = Box::pin(
        stream::iter(
            jobs.into_iter()
                .map(|job| async move { download(client, &job.url, &job.dest, job.size).await }),
        )
        .buffer_unordered(PARALLEL_DOWNLOADS),
    );

    let mut done = 0usize;
    while let Some(result) = results.next().await {
        result?;
        done += 1;
        let percent = from + (to - from) * done as f64 / total as f64;
        progress.report(stage, format!("{label} ({done}/{total})…"), percent, done, total, done == total);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Profile helpers
// ---------------------------------------------------------------------------------------------

/// Evaluates Mojang's `rules` (os name / arch). Feature-gated rules (demo mode, custom
/// resolution, quick play) are treated as not matching.
fn rules_allow(rules: Option<&Value>) -> bool {
    let Some(rules) = rules.and_then(Value::as_array) else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        if rule.get("features").is_some() {
            continue;
        }
        let allow = rule.get("action").and_then(Value::as_str) == Some("allow");
        let os_ok = match rule.get("os") {
            None => true,
            Some(os) => {
                let name_ok = os.get("name").and_then(Value::as_str).is_none_or(|n| n == OS_NAME);
                let arch_ok = os
                    .get("arch")
                    .and_then(Value::as_str)
                    .is_none_or(|a| (a == "x86") == cfg!(target_pointer_width = "32"));
                name_ok && arch_ok
            }
        };
        if os_ok {
            allowed = allow;
        }
    }
    allowed
}

fn push_args(out: &mut Vec<String>, list: &[Value], vars: &HashMap<&str, String>) {
    for entry in list {
        match entry {
            Value::String(s) => out.push(substitute(s, vars)),
            Value::Object(_) => {
                if !rules_allow(entry.get("rules")) {
                    continue;
                }
                match entry.get("value") {
                    Some(Value::String(s)) => out.push(substitute(s, vars)),
                    Some(Value::Array(items)) => {
                        out.extend(items.iter().filter_map(Value::as_str).map(|s| substitute(s, vars)))
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn substitute(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        let token = format!("${{{key}}}");
        if out.contains(&token) {
            out = out.replace(&token, value);
        }
    }
    out
}

/// `group:artifact:version[:classifier][@ext]` → `group/path/artifact/version/artifact-version[-classifier].ext`
fn maven_path(name: &str) -> Option<String> {
    let (coords, ext) = match name.split_once('@') {
        Some((c, e)) => (c, e),
        None => (name, "jar"),
    };
    let parts: Vec<&str> = coords.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[2]);
    let classifier = parts.get(3).map(|c| format!("-{c}")).unwrap_or_default();
    Some(format!(
        "{}/{artifact}/{version}/{artifact}-{version}{classifier}.{ext}",
        group.replace('.', "/")
    ))
}

/// Dedup key: everything except the version (`group:artifact[:classifier]`).
fn library_key(name: &str) -> String {
    let parts: Vec<&str> = name.split('@').next().unwrap_or(name).split(':').collect();
    match parts.len() {
        0..=2 => name.to_string(),
        3 => format!("{}:{}", parts[0], parts[1]),
        _ => format!("{}:{}:{}", parts[0], parts[1], parts[3]),
    }
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

/// Numbers inside an id, for "newest build" comparisons.
pub(crate) fn version_key(id: &str) -> Vec<u64> {
    id.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse().ok()).collect()
}

pub(crate) fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub(crate) fn sanitize_username(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(16)
        .collect();
    if clean.len() >= 3 {
        clean
    } else {
        "Player".into()
    }
}

/// UUID that offline-mode servers use: name-based (v3) UUID of "OfflinePlayer:<name>".
pub(crate) fn offline_uuid(name: &str) -> String {
    let mut bytes = md5::compute(format!("OfflinePlayer:{name}").as_bytes()).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// Integrity check («Проверить целостность файлов»)
// ---------------------------------------------------------------------------------------------

/// What the check found and fixed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairReport {
    pub version_id: String,
    /// Files compared with their checksums (client, libraries, asset index, assets).
    pub checked: usize,
    /// Present but with a wrong size or sha1.
    pub corrupt: usize,
    pub missing: usize,
    pub repaired: usize,
    /// Files that could not be downloaded again (with the reason).
    pub failed: Vec<String>,
    /// Libraries generated by the Forge/NeoForge installer that are gone (only a reinstall helps).
    pub local_missing: usize,
}

/// One file with its source and checksum. `sha1: None` → only existence and size are checked.
struct Check {
    url: String,
    dest: PathBuf,
    size: Option<u64>,
    sha1: Option<String>,
    native: bool,
}

/// Checks the installed version behind `label` against Mojang's (and the loader's) checksums and
/// downloads whatever is missing or damaged. Progress: `repair-progress`.
pub async fn repair(app: AppHandle, label: &str) -> Result<RepairReport, String> {
    repair_with(std::sync::Arc::new(Progress::new(app, "repair-progress")), &minecraft_dir(), label).await
}

pub(crate) async fn repair_with(progress: std::sync::Arc<Progress>, game_dir: &Path, label: &str) -> Result<RepairReport, String> {
    let id = resolve_installed(game_dir, label.trim())
        .ok_or("Эта версия ещё не скачана — запустите установку, и лаунчер скачает все файлы сам.")?;
    let client = http_client()?;
    progress.step("profile", "Сверка профилей версии с манифестом Mojang…", 1.0);

    // Version JSONs of the chain (child → vanilla parent).
    let mut chain = vec![id.clone()];
    while chain.len() < 6 {
        let json = read_json(&version_json(game_dir, chain.last().unwrap_or(&id)))?;
        match json.get("inheritsFrom").and_then(Value::as_str) {
            Some(parent) => chain.push(parent.to_string()),
            None => break,
        }
    }
    let mut report = RepairReport { version_id: id.clone(), checked: 0, corrupt: 0, missing: 0, repaired: 0, failed: Vec::new(), local_missing: 0 };
    // Vanilla profiles are listed in the manifest with their sha1; a damaged one is fetched again.
    if let Ok(manifest) = get_json(&client, VERSION_MANIFEST).await {
        for vid in &chain {
            let entry = manifest
                .get("versions")
                .and_then(Value::as_array)
                .and_then(|list| list.iter().find(|v| v.get("id").and_then(Value::as_str) == Some(vid.as_str())));
            let (Some(url), Some(sha1)) = (
                entry.and_then(|e| e.get("url")).and_then(Value::as_str),
                entry.and_then(|e| e.get("sha1")).and_then(Value::as_str),
            ) else {
                continue;
            };
            let path = version_json(game_dir, vid);
            report.checked += 1;
            if file_sha1(&path).as_deref() != Some(sha1) {
                report.corrupt += 1;
                let _ = fs::remove_file(&path);
                match download(&client, url, &path, None).await {
                    Ok(()) => report.repaired += 1,
                    Err(e) => report.failed.push(format!("{vid}.json: {e}")),
                }
            }
        }
    }
    let profile = load_profile(game_dir, &id, 0)?;
    let first = |ptr: &str| -> Option<Value> {
        chain.iter().find_map(|vid| read_json(&version_json(game_dir, vid)).ok()?.pointer(ptr).cloned())
    };
    let str_at = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);

    let mut checks: Vec<Check> = Vec::new();
    // client.jar
    if let Some(c) = first("/downloads/client") {
        checks.push(Check {
            url: str_at(&c, "url").unwrap_or_default(),
            dest: game_dir.join("versions").join(&profile.jar_id).join(format!("{}.jar", profile.jar_id)),
            size: c.get("size").and_then(Value::as_u64),
            sha1: str_at(&c, "sha1"),
            native: false,
        });
    }
    // libraries (same selection as ensure_libraries)
    let libs_dir = game_dir.join("libraries");
    let arch = if cfg!(target_pointer_width = "64") { "64" } else { "32" };
    let mut seen = HashSet::new();
    for lib in &profile.libraries {
        if !rules_allow(lib.get("rules")) {
            continue;
        }
        let name = lib.get("name").and_then(Value::as_str).unwrap_or("");
        if let Some(classifier) = lib.pointer(&format!("/natives/{OS_NAME}")).and_then(Value::as_str) {
            let classifier = classifier.replace("${arch}", arch);
            if let Some(d) = lib.pointer(&format!("/downloads/classifiers/{classifier}")) {
                if let (Some(url), Some(path)) = (str_at(d, "url"), str_at(d, "path")) {
                    checks.push(Check { url, dest: libs_dir.join(path), size: d.get("size").and_then(Value::as_u64), sha1: str_at(d, "sha1"), native: true });
                }
            }
        }
        let (dest, url, size, sha1) = if let Some(artifact) = lib.pointer("/downloads/artifact") {
            let Some(path) = str_at(artifact, "path") else { continue };
            (libs_dir.join(path), str_at(artifact, "url").unwrap_or_default(), artifact.get("size").and_then(Value::as_u64), str_at(artifact, "sha1"))
        } else if lib.get("natives").is_some() && lib.get("downloads").is_some() {
            continue;
        } else {
            let Some(rel) = maven_path(name) else { continue };
            let base = lib.get("url").and_then(Value::as_str).unwrap_or(LIBRARIES_URL);
            let base = if base.ends_with('/') { base.to_string() } else { format!("{base}/") };
            (libs_dir.join(&rel), format!("{base}{rel}"), lib.get("size").and_then(Value::as_u64), str_at(lib, "sha1"))
        };
        if !name.is_empty() && !seen.insert(library_key(name)) {
            continue;
        }
        if url.is_empty() {
            if !dest.is_file() {
                report.local_missing += 1;
            }
            continue;
        }
        checks.push(Check { url, dest, size, sha1, native: false });
    }
    // asset index first (the objects are read from it), then the objects
    let assets = game_dir.join("assets");
    let index_path = assets.join("indexes").join(format!("{}.json", profile.asset_index));
    if let Some(ai) = first("/assetIndex") {
        let index_check = Check {
            url: str_at(&ai, "url").unwrap_or_default(),
            dest: index_path.clone(),
            size: ai.get("size").and_then(Value::as_u64),
            sha1: str_at(&ai, "sha1"),
            native: false,
        };
        report.checked += 1;
        if !index_check.url.is_empty() && !check_ok(&index_check) {
            if index_check.dest.is_file() { report.corrupt += 1 } else { report.missing += 1 }
            let _ = fs::remove_file(&index_path);
            match download(&client, &index_check.url, &index_path, index_check.size).await {
                Ok(()) => report.repaired += 1,
                Err(e) => report.failed.push(format!("{}.json: {e}", profile.asset_index)),
            }
        }
    }
    if let Ok(index) = read_json(&index_path) {
        let mut hashes = HashSet::new();
        if let Some(objects) = index.get("objects").and_then(Value::as_object) {
            for object in objects.values() {
                let Some(hash) = object.get("hash").and_then(Value::as_str) else { continue };
                if hash.len() < 2 || !hashes.insert(hash.to_string()) {
                    continue;
                }
                checks.push(Check {
                    url: format!("{RESOURCES_URL}/{}/{hash}", &hash[..2]),
                    dest: assets.join("objects").join(&hash[..2]).join(hash),
                    size: object.get("size").and_then(Value::as_u64),
                    sha1: Some(hash.to_string()),
                    native: false,
                });
            }
        }
    }

    // Hashing thousands of files: worker threads, off the async runtime.
    let total = checks.len();
    report.checked += total;
    let checks = std::sync::Arc::new(checks);
    let bad: Vec<usize> = {
        let checks = checks.clone();
        let progress = progress.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let done = std::sync::atomic::AtomicUsize::new(0);
            let bad = Mutex::new(Vec::new());
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
            std::thread::scope(|scope| {
                for t in 0..threads {
                    let (checks, done, bad, progress) = (&checks, &done, &bad, &progress);
                    scope.spawn(move || {
                        for i in (t..checks.len()).step_by(threads) {
                            if !check_ok(&checks[i]) {
                                bad.lock().unwrap_or_else(|e| e.into_inner()).push(i);
                            }
                            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            progress.report("verify", format!("Проверка файлов ({n}/{total})…"), 3.0 + 67.0 * n as f64 / total.max(1) as f64, n, total, n == total);
                        }
                    });
                }
            });
            bad.into_inner().unwrap_or_else(|e| e.into_inner())
        })
        .await
        .map_err(|e| e.to_string())?
    };

    // Download again what is missing or damaged, then hash once more.
    let fix_total = bad.len();
    let mut natives_touched = false;
    if fix_total > 0 {
        progress.report("repair", format!("Перекачивание файлов (0/{fix_total})…"), 70.0, 0, fix_total, true);
        for &i in &bad {
            if checks[i].dest.is_file() { report.corrupt += 1 } else { report.missing += 1 }
        }
        let client = &client;
        let checks_ref = &checks;
        let mut results = Box::pin(
            stream::iter(bad.iter().copied().map(|i| async move {
                let c = &checks_ref[i];
                let _ = fs::remove_file(&c.dest);
                let result = download(client, &c.url, &c.dest, c.size).await.and_then(|_| {
                    if check_ok(c) {
                        Ok(())
                    } else {
                        let _ = fs::remove_file(&c.dest);
                        Err("контрольная сумма не совпала и после повторной загрузки".to_string())
                    }
                });
                (i, result)
            }))
            .buffer_unordered(PARALLEL_DOWNLOADS),
        );
        let mut done = 0usize;
        while let Some((i, result)) = results.next().await {
            done += 1;
            match result {
                Ok(()) => {
                    report.repaired += 1;
                    natives_touched |= checks[i].native;
                }
                Err(e) => {
                    let file = checks[i].dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    report.failed.push(format!("{file}: {e}"));
                }
            }
            progress.report("repair", format!("Перекачивание файлов ({done}/{fix_total})…"), 70.0 + 29.0 * done as f64 / fix_total as f64, done, fix_total, done == fix_total);
        }
    }
    // Natives are unpacked next to the version; a repaired natives jar is unpacked again at launch.
    if natives_touched {
        let _ = fs::remove_dir_all(game_dir.join("versions").join(&profile.id).join("natives"));
    }
    progress.step("done", "Проверка завершена", 100.0);
    Ok(report)
}

/// Size first (cheap), then sha1 when it is known.
fn check_ok(c: &Check) -> bool {
    let Ok(meta) = fs::metadata(&c.dest) else { return false };
    if !meta.is_file() || c.size.is_some_and(|s| s != meta.len()) {
        return false;
    }
    match &c.sha1 {
        Some(expected) => file_sha1(&c.dest).is_some_and(|h| h.eq_ignore_ascii_case(expected)),
        None => meta.len() > 0,
    }
}

fn file_sha1(path: &Path) -> Option<String> {
    use sha1::{Digest, Sha1};
    use std::io::Read;
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maven_paths() {
        assert_eq!(maven_path("org.ow2.asm:asm:9.6").unwrap(), "org/ow2/asm/asm/9.6/asm-9.6.jar");
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.3:natives-windows").unwrap(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"
        );
    }

    #[test]
    fn library_keys_ignore_version() {
        assert_eq!(library_key("org.ow2.asm:asm:9.6"), library_key("org.ow2.asm:asm:9.7"));
        assert_ne!(library_key("org.lwjgl:lwjgl:3.3.3"), library_key("org.lwjgl:lwjgl:3.3.3:natives-windows"));
    }

    #[test]
    fn rules_follow_os_and_skip_features() {
        let this_os = json!([{ "action": "allow", "os": { "name": OS_NAME } }]);
        let other_os = json!([{ "action": "allow", "os": { "name": "beos" } }]);
        let feature = json!([{ "action": "allow", "features": { "is_demo_user": true } }]);
        assert!(rules_allow(None));
        assert!(rules_allow(Some(&this_os)));
        assert!(!rules_allow(Some(&other_os)));
        assert!(!rules_allow(Some(&feature)));
    }

    #[test]
    fn labels_map_to_loaders() {
        assert_eq!(parse_label("Release 1.21.9"), (Loader::Vanilla, Some("1.21.9".into())));
        assert_eq!(parse_label("Fabric 1.21.1"), (Loader::Fabric, Some("1.21.1".into())));
        assert_eq!(parse_label("Forge 1.20.1"), (Loader::Forge, Some("1.20.1".into())));
        assert_eq!(parse_label("NeoForge 1.21.1"), (Loader::NeoForge, Some("1.21.1".into())));
        assert_eq!(parse_label("Snapshot 26.4-snapshot-3"), (Loader::Vanilla, Some("26.4-snapshot-3".into())));
        assert_eq!(parse_label("Beta b1.7.3"), (Loader::Vanilla, Some("b1.7.3".into())));
        assert_eq!(parse_label("Alpha a1.2.6"), (Loader::Vanilla, Some("a1.2.6".into())));
        assert_eq!(parse_label("Последний релиз"), (Loader::Vanilla, None));
    }

    #[test]
    fn finds_installed_loaders() {
        let dir = std::env::temp_dir().join(format!("signal-resolve-test-{}", std::process::id()));
        for id in ["1.21.10-forge-60.1.0", "1.21.1-forge-52.1.0", "neoforge-21.1.99", "neoforge-21.1.256", "neoforge-26.3.0.57-beta"] {
            let v = dir.join("versions").join(id);
            fs::create_dir_all(&v).unwrap();
            fs::write(v.join(format!("{id}.json")), "{}").unwrap();
        }
        assert_eq!(resolve_installed(&dir, "Forge 1.21.1").as_deref(), Some("1.21.1-forge-52.1.0"));
        assert_eq!(resolve_installed(&dir, "NeoForge 1.21.1").as_deref(), Some("neoforge-21.1.256"));
        assert_eq!(resolve_installed(&dir, "NeoForge 26.3").as_deref(), Some("neoforge-26.3.0.57-beta"));
        assert_eq!(resolve_installed(&dir, "Forge 1.20.1"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_java_versions() {
        assert_eq!(parse_java_major(r#"openjdk version "21.0.7" 2025-04-15 LTS"#), Some(21));
        assert_eq!(parse_java_major(r#"java version "1.8.0_382""#), Some(8));
        assert_eq!(parse_java_major(r#"openjdk version "17" 2021-09-14"#), Some(17));
        assert!(java_fits(8, 8) && !java_fits(17, 8));
        assert!(java_fits(21, 17) && !java_fits(17, 21));
    }

    #[test]
    fn offline_uuid_is_v3() {
        assert_eq!(offline_uuid("Notch"), "b50ad385829d3141a2167e7d7539ba7f");
    }

    #[test]
    fn substitutes_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("version_name", "1.21.1".to_string());
        assert_eq!(substitute("--version ${version_name}", &vars), "--version 1.21.1");
    }

    /// Damages one asset of a real installed version, lets the integrity check repair it.
    /// `cargo test --lib live_repair -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_repair() {
        let game_dir = minecraft_dir();
        let label = "Release 1.21.1";
        let id = resolve_installed(&game_dir, label).expect("1.21.1 is not installed");
        let profile = load_profile(&game_dir, &id, 0).unwrap();
        let index = read_json(&game_dir.join("assets").join("indexes").join(format!("{}.json", profile.asset_index))).unwrap();
        let hash = index["objects"].as_object().unwrap().values().find_map(|o| o["hash"].as_str().map(str::to_string)).unwrap();
        let victim = game_dir.join("assets").join("objects").join(&hash[..2]).join(&hash);
        let original = fs::read(&victim).unwrap();
        fs::write(&victim, b"broken").unwrap();
        let report = tauri::async_runtime::block_on(repair_with(std::sync::Arc::new(Progress::console()), &game_dir, label));
        if fs::read(&victim).map(|b| b != original).unwrap_or(true) {
            fs::write(&victim, &original).unwrap();
        }
        let report = report.unwrap();
        println!("{report:?}");
        assert!(report.corrupt >= 1 && report.repaired >= 1 && report.failed.is_empty());
        assert_eq!(file_sha1(&victim).as_deref(), Some(hash.as_str()));
    }

    /// Read-only run over a Fabric installation (nothing is damaged on purpose).
    /// `cargo test --lib live_verify_fabric -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_verify_fabric() {
        let report = tauri::async_runtime::block_on(repair_with(std::sync::Arc::new(Progress::console()), &minecraft_dir(), "Fabric 1.21.1")).unwrap();
        println!("{report:?}");
        assert!(report.checked > 100 && report.failed.is_empty());
    }

    /// Which Java installations the launcher sees: `cargo test --lib live_list_java -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_list_java() {
        for j in list_java() {
            println!("Java {:>2} · {:<6} · {}", j.major, j.source, j.path);
        }
        assert!(check_java(r"C:\Windows
otepad.exe").is_err());
    }

    #[test]
    fn java_fit_rules() {
        assert!(java_fits(8, 8) && !java_fits(17, 8) && !java_fits(21, 8));
        assert!(java_fits(21, 17) && java_fits(17, 17) && !java_fits(8, 17));
    }
}
