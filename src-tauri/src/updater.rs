//! Self-update through the official `tauri-plugin-updater`.
//!
//! The Signal server publishes `update.json`: which version is out and where its signed installer is.
//! Every installer is checked against the public key in `tauri.conf.json` (`plugins.updater.pubkey`)
//! and must be signed for exactly the version it is announced as (`requireSignedVersion`), so a
//! tampered or downgraded file is refused.
//!
//! The whole flow stays in Rust: the page can ask to check and to install, but never chooses
//! what gets installed.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The release feed on the Signal server; the same address as `plugins.updater.endpoints` in tauri.conf.json.
pub const UPDATE_URL: &str = "https://signallauncher.mooo.com/update.json";

/// Where `update.json` is: [`UPDATE_URL`], or `SIGNAL_UPDATE_URL` (at build time or at runtime) to test
/// against another feed. Every build has a source, so «Проверить обновления» always asks a server.
pub fn endpoint() -> Option<String> {
    std::env::var("SIGNAL_UPDATE_URL")
        .ok()
        .or_else(|| option_env!("SIGNAL_UPDATE_URL").map(str::to_string))
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| Some(UPDATE_URL.to_string()))
}

/// The update found by the last check, kept until «Обновить сейчас».
#[derive(Default)]
pub struct UpdateState {
    pending: Mutex<Option<Update>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// An update source is built in.
    pub configured: bool,
    pub available: bool,
    pub current: String,
    pub version: Option<String>,
    /// Release notes from `update.json`.
    pub notes: Option<String>,
    /// `YYYY-MM-DD`
    pub date: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
}

pub async fn check(app: &AppHandle, state: &UpdateState) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let Some(url) = endpoint() else {
        return Ok(UpdateInfo { configured: false, available: false, current, version: None, notes: None, date: None });
    };
    let url = url.parse().map_err(|e| format!("Некорректный адрес обновлений: {e}"))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .map_err(|e| e.to_string())?
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let found = updater.check().await.map_err(|e| format!("Не удалось проверить обновления: {e}"))?;
    let info = UpdateInfo {
        configured: true,
        available: found.is_some(),
        current,
        version: found.as_ref().map(|u| u.version.clone()),
        notes: found.as_ref().and_then(|u| u.body.clone()).filter(|b| !b.trim().is_empty()),
        date: found.as_ref().and_then(|u| u.date).map(|d| d.date().to_string()),
    };
    *state.pending.lock().unwrap_or_else(|e| e.into_inner()) = found;
    Ok(info)
}

/// Downloads (progress: `update-progress`), verifies the signature and runs the installer.
/// On Windows the installer closes the launcher and starts the new version itself; elsewhere
/// the app restarts.
pub async fn install(app: &AppHandle, state: &UpdateState) -> Result<(), String> {
    let update = state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or("Сначала проверьте обновления")?;
    let mut downloaded = 0u64;
    let progress_app = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress_app.emit("update-progress", UpdateProgress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| format!("Обновление не установилось: {e}"))?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in feed and tauri.conf.json must not drift apart.
    #[test]
    fn feed_matches_tauri_conf() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let endpoints = conf.pointer("/plugins/updater/endpoints").and_then(|e| e.as_array()).unwrap();
        assert!(endpoints.iter().any(|e| e.as_str() == Some(UPDATE_URL)));
        if std::env::var("SIGNAL_UPDATE_URL").is_err() && option_env!("SIGNAL_UPDATE_URL").is_none() {
            assert_eq!(endpoint().as_deref(), Some(UPDATE_URL));
        }
    }

    /// Reads the live feed the way a 0.3.x launcher does: `cargo test --lib live_update_feed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_update_feed() {
        let feed: serde_json::Value = tauri::async_runtime::block_on(async {
            reqwest::get(UPDATE_URL).await.unwrap().error_for_status().unwrap().json().await.unwrap()
        });
        let version = feed["version"].as_str().unwrap();
        let nsis = &feed["platforms"]["windows-x86_64-nsis"];
        println!("{UPDATE_URL}: {version} → {}", nsis["url"]);
        assert!(version.split('.').count() == 3 && version.split('.').all(|p| p.parse::<u32>().is_ok()));
        assert!(nsis["signature"].as_str().is_some_and(|s| !s.is_empty()));
    }
}
