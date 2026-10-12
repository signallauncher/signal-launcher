mod auth;
mod console;
mod curseforge;
mod discord;
mod export;
mod game;
mod gamedir;
mod instances;
mod loaders;
mod modrinth;
mod mods;
mod mrpack;
mod news;
mod offline_skin;
mod packs;
mod saves;
mod servers;
mod skinpack;
mod system;
mod updater;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::json;

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const OPENROUTER_MODEL: &str = "openrouter/free";
const SYSTEM_PROMPT: &str = "Ты — сборщик модпаков Minecraft. Отвечай СТРОГО одним JSON-объектом без markdown и пояснений: \
{\"title\": string, \"description\": string (1–2 предложения по-русски), \"loader\": \"Fabric\" | \"Forge\" | \"NeoForge\", \
\"version\": string (например \"1.21.1\"), \"mods\": [{\"name\": string (реальное название мода с Modrinth или CurseForge), \
\"category\": string (по-русски, 1–2 слова), \"description\": string (одно короткое предложение по-русски)}]}. \
Подбери 8–14 реальных, совместимых между собой модов под выбранный загрузчик и версию, обязательно добавь нужные библиотеки \
(например, Fabric API для Fabric).";

/// The OpenRouter key lives only in the backend: taken from the OPENROUTER_API_KEY environment
/// variable at runtime, or baked in at compile time if the variable was set during `tauri build`.
fn openrouter_key() -> Option<String> {
    std::env::var("OPENROUTER_API_KEY")
        .ok()
        .or_else(|| option_env!("OPENROUTER_API_KEY").map(str::to_string))
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// Expands Windows-style `%VAR%` segments, e.g. `%APPDATA%\.minecraft`. `%MINECRAFT%` is the
/// current game folder (it can be moved), so the UI never hardcodes where it is.
fn expand_path(raw: &str) -> PathBuf {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                let value = if name.eq_ignore_ascii_case("MINECRAFT") {
                    Ok(game::path_str(&game::minecraft_dir()))
                } else {
                    std::env::var(name)
                };
                match value {
                    Ok(value) => out.push_str(&value),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    PathBuf::from(out)
}

/// «ИГРАТЬ». Downloads whatever the version is missing from Mojang (client, libraries, assets,
/// Java), reporting `download-progress` events, then starts the game with `javaw`.
/// When the game exits, `game-exit` is emitted (see `game::watch_game`).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn launch_minecraft(
    app: tauri::AppHandle,
    version: String,
    instance_id: Option<String>,
    instance_name: Option<String>,
    username: Option<String>,
    ram_gb: Option<u32>,
    jvm_args: Option<String>,
    priority: Option<String>,
    hide_launcher: Option<bool>,
    backup: Option<String>,
    account_id: Option<String>,
    discord: Option<bool>,
    server: Option<String>,
    skin: Option<skinpack::SkinPayload>,
    java_path: Option<String>,
) -> Result<game::LaunchOutcome, String> {
    // Microsoft account: fresh tokens before anything is downloaded, so a dead session fails fast.
    let auth = match account_id.filter(|id| !id.is_empty()) {
        Some(id) => Some(auth::session(&app, &id).await?),
        None => None,
    };
    let instance_id = instance_id.filter(|id| !id.trim().is_empty()).unwrap_or_else(|| "default".into());
    let opts = game::LaunchOptions {
        instance_name: instance_name.unwrap_or_else(|| instance_id.clone()),
        instance_id,
        version_label: version,
        username: username.unwrap_or_default(),
        ram_gb: ram_gb.unwrap_or(4),
        extra_jvm_args: jvm_args
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        priority: game::Priority::parse(priority.as_deref().unwrap_or("normal")),
        hide_launcher: hide_launcher.unwrap_or(false),
        backup: backup.filter(|f| !f.is_empty()),
        auth,
        discord: discord.unwrap_or(false),
        server,
        skin,
        java_path: java_path.filter(|p| !p.trim().is_empty()),
    };
    game::launch(app, opts).await
}

/// «Собрать с ИИ». Sends the prompt to OpenRouter and returns the model's raw text
/// (the frontend parses the JSON and falls back to a preset on any error).
#[tauri::command]
async fn generate_ai_modpack(prompt: String) -> Result<String, String> {
    let key = openrouter_key().ok_or("OPENROUTER_API_KEY не задан")?;
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Пустой запрос".into());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let body = json!({
        "model": OPENROUTER_MODEL,
        "temperature": 0.6,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": prompt }
        ]
    });

    let response = client
        .post(OPENROUTER_URL)
        .bearer_auth(key)
        .header("HTTP-Referer", "https://signallauncher.app")
        .header("X-Title", "Signal Launcher")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("сеть недоступна: {e}"))?;

    let status = response.status();
    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("некорректный ответ: {e}"))?;

    if !status.is_success() {
        let message = data
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("неизвестная ошибка");
        return Err(format!("HTTP {}: {message}", status.as_u16()));
    }

    data.pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "пустой ответ модели".to_string())
}

/// «Папка игры», «Открыть .minecraft», screenshots: opens a folder in the file manager or a file
/// in its default app. Missing folders inside `.minecraft` (e.g. an installation's `screenshots`)
/// are created first.
#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    let dir = expand_path(path.trim());
    if !dir.exists() && dir.starts_with(game::minecraft_dir()) {
        let _ = std::fs::create_dir_all(&dir);
    }
    if !dir.exists() {
        return Err(format!("Не найдено: {}", dir.display()));
    }

    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";

    Command::new(program)
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("Не удалось открыть {}: {e}", dir.display()))?;
    Ok(())
}

/// «Установить» in the catalog and «Скачать и установить сборку»: mods from Modrinth into
/// `.minecraft/instances/<id>/mods`, with required dependencies. Progress: `mods-progress`.
#[tauri::command]
async fn install_mods(
    app: tauri::AppHandle,
    instance_id: String,
    instance_name: Option<String>,
    version: String,
    mods: Vec<mods::ModRequest>,
) -> Result<mods::InstallReport, String> {
    let name = instance_name.unwrap_or_else(|| instance_id.clone());
    mods::install(app, instance_id, name, version, mods).await
}

#[tauri::command]
fn list_mods(instance_id: String) -> Vec<mods::ModFile> {
    mods::list(&instance_id)
}

#[tauri::command]
fn remove_mod(instance_id: String, file: String) -> Result<(), String> {
    mods::remove(&instance_id, &file)
}

/// Mod manager switch: `.jar` ↔ `.jar.disabled`. Returns the new file name.
#[tauri::command]
fn set_mod_enabled(instance_id: String, file: String, enabled: bool) -> Result<String, String> {
    mods::set_enabled(&instance_id, &file, enabled)
}

/// Game versions from Mojang's manifest (cached for offline use).
#[tauri::command]
async fn list_versions() -> Result<game::VersionList, String> {
    game::list_versions().await
}

/// «Перенести в установку…»: copies a world into `.minecraft/instances/<id>/saves`.
#[tauri::command]
async fn transfer_world(
    path: String,
    instance_id: String,
    instance_name: Option<String>,
    version: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        saves::transfer_world(&path, &instance_id, instance_name.as_deref(), version.as_deref())
    })
        .await
        .map_err(|e| e.to_string())?
}

/// Launcher version and whether this build has an update source.
#[tauri::command]
fn launcher_info(app: tauri::AppHandle) -> serde_json::Value {
    json!({ "version": app.package_info().version.to_string(), "updatesConfigured": updater::endpoint().is_some() })
}

/// «Проверить обновления лаунчера» (and the check on start).
#[tauri::command]
async fn check_launcher_update(app: tauri::AppHandle, state: tauri::State<'_, updater::UpdateState>) -> Result<updater::UpdateInfo, String> {
    updater::check(&app, &state).await
}

/// «Обновить сейчас»: download (`update-progress`), signature check, install, restart.
#[tauri::command]
async fn install_launcher_update(app: tauri::AppHandle, state: tauri::State<'_, updater::UpdateState>) -> Result<(), String> {
    updater::install(&app, &state).await
}

/// Which optional integrations this build can use (ids come from env / build time).
#[tauri::command]
fn integrations() -> serde_json::Value {
    json!({ "microsoft": auth::client_id().is_some(), "discord": discord::app_id().is_some() })
}

/// «Войти через Microsoft», step 1: the code to type on microsoft.com/link.
#[tauri::command]
async fn ms_login_start(state: tauri::State<'_, auth::LoginState>) -> Result<auth::DeviceCode, String> {
    auth::start(&state).await
}

/// Step 2: waits until the code is confirmed in the browser, then signs in to Xbox and Minecraft.
#[tauri::command]
async fn ms_login_finish(app: tauri::AppHandle, state: tauri::State<'_, auth::LoginState>) -> Result<auth::AccountInfo, String> {
    auth::finish(&app, &state).await
}

#[tauri::command]
fn ms_login_cancel(state: tauri::State<'_, auth::LoginState>) {
    auth::cancel(&state);
}

#[tauri::command]
fn list_accounts(app: tauri::AppHandle) -> Vec<auth::AccountInfo> {
    auth::list(&app)
}

#[tauri::command]
fn remove_account(app: tauri::AppHandle, id: String) -> Result<(), String> {
    auth::remove(&app, &id)
}

/// Opens Microsoft's sign-in page in the default browser. Only that page is allowed.
#[tauri::command]
fn open_login_page(url: String) -> Result<(), String> {
    let allowed = ["https://www.microsoft.com/link", "https://microsoft.com/link", "https://login.microsoftonline.com/"];
    if !allowed.iter().any(|prefix| url.starts_with(prefix)) {
        return Err("Недопустимый адрес".into());
    }
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer").arg(&url).spawn();
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(&url).spawn();
    result.map(|_| ()).map_err(|e| format!("Не удалось открыть браузер: {e}"))
}

/// Console lines kept for an installation (last 5000): for a console opened after the launch.
#[tauri::command]
fn console_snapshot(console: tauri::State<'_, console::Console>, instance_id: String) -> Vec<console::LogLine> {
    console.snapshot(&instance_id)
}

/// «Серверы»: Server List Ping (online, players, latency, MOTD, icon).
#[tauri::command]
async fn ping_server(address: String) -> servers::ServerStatus {
    servers::ping(&address).await
}

/// Servers added in game (`servers.dat` of the shared folder and every installation).
#[tauri::command]
async fn saved_servers() -> Result<Vec<servers::SavedServer>, String> {
    tauri::async_runtime::spawn_blocking(servers::saved)
        .await
        .map_err(|e| e.to_string())
}

/// «Новости»: Java Edition news and patch notes (cached for 30 minutes and for offline use).
#[tauri::command]
async fn news_feed(force: Option<bool>) -> Result<news::NewsFeed, String> {
    news::feed(force.unwrap_or(false)).await
}

#[tauri::command]
async fn news_article(content_path: String) -> Result<String, String> {
    news::article(&content_path).await
}

/// Opens an article on minecraft.net (or Modrinth) in the default browser.
#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    if !news::allowed_link(&url) {
        return Err("Эту ссылку лаунчер не открывает".into());
    }
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer").arg(&url).spawn();
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(&url).spawn();
    result.map(|_| ()).map_err(|e| format!("Не удалось открыть браузер: {e}"))
}

/// A skin dropped on the window: Tauri gives a path, the skin editor needs the bytes.
/// Only `.png` files up to 1 MB.
#[tauri::command]
fn read_skin_png(path: String) -> Result<tauri::ipc::Response, String> {
    let path = PathBuf::from(path);
    let is_png = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err("Скин — это файл .png".into());
    }
    let size = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > 1024 * 1024 {
        return Err("Файл слишком большой для скина (больше 1 МБ)".into());
    }
    std::fs::read(&path).map(tauri::ipc::Response::new).map_err(|e| format!("{}: {e}", path.display()))
}

/// «Сменить папку…»: where the game folder is.
#[tauri::command]
fn game_dir_info() -> gamedir::GameDirInfo {
    gamedir::info()
}

/// What the picked folder contains (empty / a Minecraft folder / other files).
#[tauri::command]
fn check_game_dir(path: String) -> gamedir::FolderCheck {
    gamedir::check(&path)
}

/// `mode`: `use`, `copy` or `move`. Progress: `gamedir-progress`.
#[tauri::command]
async fn change_game_dir(app: tauri::AppHandle, path: String, mode: String) -> Result<gamedir::GameDirInfo, String> {
    tauri::async_runtime::spawn_blocking(move || gamedir::change(&app, &path, &mode))
        .await
        .map_err(|e| e.to_string())?
}

/// Space taken by versions, libraries, assets, Java, installations… and free space on the disk.
#[tauri::command]
async fn disk_usage() -> Result<gamedir::DiskUsage, String> {
    tauri::async_runtime::spawn_blocking(gamedir::usage)
        .await
        .map_err(|e| e.to_string())
}

/// Live Modrinth catalog: mods, modpacks, resource packs, shaders.
#[tauri::command]
async fn search_modrinth(query: modrinth::SearchQuery) -> Result<modrinth::SearchResult, String> {
    modrinth::search(query).await
}

/// One-click install of a resource pack or shader into the installation.
#[tauri::command]
async fn install_asset(instance_id: String, version: String, project_id: String, kind: String) -> Result<modrinth::AssetInstall, String> {
    modrinth::install_asset(&instance_id, &version, &project_id, &kind).await
}

/// A modpack from the catalog becomes a new installation. Progress: `import-progress`.
#[tauri::command]
async fn install_modpack_project(app: tauri::AppHandle, project_id: String) -> Result<mrpack::ImportReport, String> {
    modrinth::install_modpack(app, &project_id).await
}

/// Resource packs / shaders of an installation (`kind`: `resourcepack` | `shader`).
#[tauri::command]
fn list_packs(instance_id: String, kind: String) -> Result<packs::PackList, String> {
    Ok(packs::list(&instance_id, packs::Kind::parse(&kind)?))
}

#[tauri::command]
fn set_pack_enabled(instance_id: String, kind: String, name: String, enabled: bool) -> Result<(), String> {
    packs::set_enabled(&instance_id, packs::Kind::parse(&kind)?, &name, enabled)
}

#[tauri::command]
fn remove_pack(instance_id: String, kind: String, name: String) -> Result<(), String> {
    packs::remove(&instance_id, packs::Kind::parse(&kind)?, &name)
}

/// «Экспорт в .mrpack», step 1: what can go into the pack and what Modrinth knows.
#[tauri::command]
async fn export_preview(instance_id: String) -> Result<export::ExportPreview, String> {
    export::preview(&instance_id).await
}

/// Step 2: writes the .mrpack. Progress: `export-progress`.
#[tauri::command]
async fn export_mrpack(
    app: tauri::AppHandle,
    instance_id: String,
    dest: String,
    meta: export::ExportMeta,
    include: Vec<String>,
) -> Result<export::ExportReport, String> {
    export::export(app, &instance_id, &dest, meta, include).await
}

/// Offline account: the skin of this nickname (Mojang, else Ely.by); `null` — none found.
#[tauri::command]
async fn resolve_offline_skin(nickname: String) -> Result<Option<offline_skin::OfflineSkin>, String> {
    offline_skin::resolve_offline_skin(&nickname).await
}

/// Which player model the game gives an offline nickname (`slim` | `wide`).
#[tauri::command]
fn offline_skin_model(name: String, version: String) -> String {
    let (_, mc) = game::parse_label(&version);
    skinpack::offline_model(&name, &mc.unwrap_or(version)).to_string()
}

/// «Бэкапы миров»: worlds of the installation and every backup made of them.
#[tauri::command]
async fn instance_backups(instance_id: String) -> Result<saves::InstanceBackups, String> {
    tauri::async_runtime::spawn_blocking(move || saves::instance_backups(&instance_id)).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn backup_instance_world(instance_id: String, folder: String) -> Result<saves::BackupEntry, String> {
    tauri::async_runtime::spawn_blocking(move || saves::backup_instance_world(&instance_id, &folder)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn restore_world_backup(instance_id: String, path: String) -> Result<saves::RestoreReport, String> {
    tauri::async_runtime::spawn_blocking(move || saves::restore_backup(&instance_id, &path)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn delete_world_backup(instance_id: String, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || saves::delete_backup(&instance_id, &path)).await.map_err(|e| e.to_string())?
}

/// «Проверить целостность файлов»: sha1 of client, libraries and assets, damaged files fetched
/// again. Progress: `repair-progress`.
#[tauri::command]
async fn repair_instance(app: tauri::AppHandle, version: String) -> Result<game::RepairReport, String> {
    game::repair(app, &version).await
}

/// CurseForge catalog: same query shape as `search_modrinth`.
#[tauri::command]
async fn search_curseforge(query: modrinth::SearchQuery) -> Result<curseforge::CfSearchResult, String> {
    curseforge::search(query).await
}

/// Mod (with required dependencies), resource pack or shader from CurseForge.
#[tauri::command]
async fn install_curseforge(
    instance_id: String,
    instance_name: String,
    version: String,
    project_id: String,
    kind: String,
) -> Result<mods::InstallReport, String> {
    curseforge::install(&instance_id, &instance_name, &version, &project_id, &kind).await
}

/// CurseForge modpack → new installation. Progress: `import-progress`.
#[tauri::command]
async fn install_curseforge_modpack(app: tauri::AppHandle, project_id: String) -> Result<curseforge::CfImportReport, String> {
    curseforge::install_modpack(app, &project_id).await
}

/// «Путь к Java»: downloaded Mojang runtimes and system Java installations (runs `java -version`).
#[tauri::command]
async fn list_java() -> Result<Vec<game::JavaInfo>, String> {
    tauri::async_runtime::spawn_blocking(game::list_java).await.map_err(|e| e.to_string())
}

/// «Указать путь вручную…»: checks that the file is a working Java.
#[tauri::command]
async fn check_java(path: String) -> Result<game::JavaInfo, String> {
    tauri::async_runtime::spawn_blocking(move || game::check_java(&path)).await.map_err(|e| e.to_string())?
}

/// «Параметры запуска» of one installation (its own RAM, Java and JVM flags).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn set_instance_launch(id: String, name: String, version: String, custom: bool, ram_gb: u32, java: String, jvm_args: String) -> Result<(), String> {
    instances::set_launch(&id, &name, &version, custom, ram_gb, &java, &jvm_args)
}

/// Installations from `.minecraft/instances/*/instance.json`.
#[tauri::command]
async fn list_instances() -> Result<Vec<instances::InstanceInfo>, String> {
    tauri::async_runtime::spawn_blocking(instances::list)
        .await
        .map_err(|e| e.to_string())
}

/// «Импорт сборки»: a Modrinth .mrpack becomes a new installation. Progress: `import-progress`.
#[tauri::command]
async fn import_modpack(app: tauri::AppHandle, path: String) -> Result<mrpack::ImportReport, String> {
    mrpack::import(app, path).await
}

/// «Проверить обновления» in the mod manager.
#[tauri::command]
async fn check_mod_updates(instance_id: String) -> Result<mods::UpdateReport, String> {
    mods::check_updates(&instance_id).await
}

#[tauri::command]
async fn update_mod(instance_id: String, file: String, version_id: String) -> Result<String, String> {
    mods::update(&instance_id, &file, &version_id).await
}

#[tauri::command]
fn rename_instance(id: String, name: String) -> Result<(), String> {
    instances::rename(&id, &name)
}

#[tauri::command]
async fn duplicate_instance(id: String, name: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || instances::duplicate(&id, &name))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn delete_instance(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || instances::delete(&id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn create_instance(id: String, name: String, version: String) -> Result<(), String> {
    instances::create(&id, &name, &version)
}

/// Total and free RAM for the memory slider.
#[tauri::command]
fn system_memory() -> system::MemoryInfo {
    system::memory()
}

/// «Очистить кэш»: unfinished downloads and extracted natives.
#[tauri::command]
async fn clear_cache() -> Result<system::CacheReport, String> {
    tauri::async_runtime::spawn_blocking(system::clear_cache)
        .await
        .map_err(|e| e.to_string())
}

/// Worlds from `saves/` of the shared game folder and every installation.
#[tauri::command]
async fn list_worlds() -> Result<Vec<saves::WorldInfo>, String> {
    tauri::async_runtime::spawn_blocking(saves::list_worlds)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_screenshots() -> Result<Vec<saves::ScreenshotInfo>, String> {
    tauri::async_runtime::spawn_blocking(saves::list_screenshots)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn backup_world(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || saves::backup_world(&path))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::UpdateState::default())
        .manage(auth::LoginState::default())
        .manage(console::Console::default())
        .setup(|app| {
            gamedir::init(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            launch_minecraft,
            generate_ai_modpack,
            open_folder,
            install_mods,
            list_mods,
            remove_mod,
            set_mod_enabled,
            list_versions,
            transfer_world,
            integrations,
            search_modrinth,
            list_java,
            check_java,
            set_instance_launch,
            search_curseforge,
            install_curseforge,
            install_curseforge_modpack,
            offline_skin_model,
            resolve_offline_skin,
            instance_backups,
            backup_instance_world,
            restore_world_backup,
            delete_world_backup,
            repair_instance,
            install_asset,
            install_modpack_project,
            list_packs,
            set_pack_enabled,
            remove_pack,
            export_preview,
            export_mrpack,
            game_dir_info,
            check_game_dir,
            change_game_dir,
            disk_usage,
            launcher_info,
            check_launcher_update,
            install_launcher_update,
            ping_server,
            saved_servers,
            news_feed,
            news_article,
            open_link,
            read_skin_png,
            console_snapshot,
            ms_login_start,
            ms_login_finish,
            ms_login_cancel,
            list_accounts,
            remove_account,
            open_login_page,
            list_instances,
            import_modpack,
            check_mod_updates,
            update_mod,
            rename_instance,
            duplicate_instance,
            delete_instance,
            create_instance,
            system_memory,
            clear_cache,
            list_worlds,
            list_screenshots,
            backup_world
        ])
        .run(tauri::generate_context!())
        .expect("error while running Signal Launcher");
}

#[cfg(test)]
mod tests {
    use super::expand_path;

    #[test]
    fn expands_known_variables() {
        std::env::set_var("SIGNAL_TEST_DIR", "C:\\Games");
        assert_eq!(
            expand_path(r"%SIGNAL_TEST_DIR%\.minecraft"),
            std::path::PathBuf::from(r"C:\Games\.minecraft")
        );
    }

    #[test]
    fn keeps_unknown_variables() {
        assert_eq!(
            expand_path(r"%SIGNAL_NO_SUCH_VAR%\x"),
            std::path::PathBuf::from(r"%SIGNAL_NO_SUCH_VAR%\x")
        );
    }
}
