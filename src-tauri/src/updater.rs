//! Self-update through the official `tauri-plugin-updater`.
//!
//! Releases live on GitHub: `latest.json` of the newest release says which version is out and
//! where its signed installer is. Every installer is checked against the public key in
//! `tauri.conf.json` (`plugins.updater.pubkey`) and must be signed for exactly the version it is
//! announced as (`requireSignedVersion`), so a tampered or downgraded file is refused.
//!
//! The whole flow stays in Rust: the page can ask to check and to install, but never chooses
//! what gets installed.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Where `latest.json` is: `SIGNAL_UPDATE_URL`, else the latest release of the GitHub repository
/// `SIGNAL_GITHUB_REPO` (`owner/repo`). Read at build time (part of the release) or at runtime
/// (for testing against another feed).
pub fn endpoint() -> Option<String> {
    let url = std::env::var("SIGNAL_UPDATE_URL").ok().or_else(|| option_env!("SIGNAL_UPDATE_URL").map(str::to_string));
    let repo = std::env::var("SIGNAL_GITHUB_REPO").ok().or_else(|| option_env!("SIGNAL_GITHUB_REPO").map(str::to_string));
    url.map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| {
            repo.map(|r| r.trim().trim_matches('/').to_string())
                .filter(|r| r.split('/').count() == 2 && !r.contains(' '))
                .map(|r| format!("https://github.com/{r}/releases/latest/download/latest.json"))
        })
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
    /// Release notes from the GitHub release.
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
