//! Discord Rich Presence: «В игре: <установка> (<версия>)» while Minecraft runs.
//! Needs a Discord application id in `SIGNAL_DISCORD_APP_ID` (runtime or build time).
//! Discord not running or no id — silently nothing.

use std::sync::Mutex;

use discord_rich_presence::activity::{Activity, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

static CLIENT: Mutex<Option<DiscordIpcClient>> = Mutex::new(None);

pub fn app_id() -> Option<String> {
    std::env::var("SIGNAL_DISCORD_APP_ID")
        .ok()
        .or_else(|| option_env!("SIGNAL_DISCORD_APP_ID").map(str::to_string))
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// Sets the status on a background thread (connecting to Discord can take a moment).
pub fn set_playing(instance: String, version: String, started_ms: i64) {
    let Some(id) = app_id() else { return };
    std::thread::spawn(move || {
        let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let mut client = DiscordIpcClient::new(&id);
            if client.connect().is_err() {
                return;
            }
            *guard = Some(client);
        }
        let details = format!("В игре: {instance} ({version})");
        let activity = Activity::new()
            .details(details.as_str())
            .state("Signal Launcher")
            .timestamps(Timestamps::new().start(started_ms));
        if let Some(client) = guard.as_mut() {
            if client.set_activity(activity).is_err() {
                // Discord was restarted: drop the stale connection, the next launch reconnects.
                *guard = None;
            }
        }
    });
}

pub fn clear() {
    std::thread::spawn(|| {
        let mut guard = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut client) = guard.take() {
            let _ = client.clear_activity();
            let _ = client.close();
        }
    });
}
