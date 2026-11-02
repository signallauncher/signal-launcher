//! Friends' live statuses: one WebSocket to the Signal server (server/src/presence.rs) for the selected profile.
//!
//! The socket reconnects on its own (1 s, 2 s, 4 s … up to a minute), pings every 20 s and reports the
//! launcher's status: `in_menu`, `playing_singleplayer` while a game runs, `host_lan` once the world is
//! opened to LAN (lanhost.rs). The UI gets two events:
//! `presence-state` (`{ state: connecting | online | reconnecting | unclaimed | off, retryIn?, error? }`)
//! and `presence-event` (what the server sent: `hello`, `presence`, `friends_changed`, `hosting`).
//! `relay_open` stays here: it becomes a data connection to the relay (`lanhost::pipe`).
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};

use crate::companion::{self, ProfileRef};

const HEARTBEAT: Duration = Duration::from_secs(20);
/// The server pings every 25 s; nothing for this long means the connection is dead.
const SILENT_LIMIT: Duration = Duration::from_secs(55);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_MAX_S: u64 = 60;

/// What the launcher is doing: the status, a short note (the game version) and the world opened to LAN.
#[derive(Debug, Clone, PartialEq)]
struct Local {
    status: &'static str,
    detail: Option<String>,
    host: Option<Host>,
}

/// A world open to LAN: the game's local port, the level's name and the UPnP address, if the router gave one.
#[derive(Debug, Clone, PartialEq)]
struct Host {
    port: u16,
    world: Option<String>,
    direct: Option<String>,
}

impl Local {
    fn message(&self) -> String {
        match &self.host {
            Some(h) => json!({ "type": "host_lan", "port": h.port, "world": h.world, "direct": h.direct }),
            None => json!({ "type": "status", "status": self.status, "detail": self.detail }),
        }
        .to_string()
    }
}

fn local() -> &'static watch::Sender<Local> {
    static LOCAL: OnceLock<watch::Sender<Local>> = OnceLock::new();
    LOCAL.get_or_init(|| watch::Sender::new(Local { status: "in_menu", detail: None, host: None }))
}

/// The running connection: the profile's UUID and its task.
static RUNNING: Mutex<Option<(String, tauri::async_runtime::JoinHandle<()>)>> = Mutex::new(None);

/// A game started (`detail`: «Fabric 1.21.1»).
pub fn playing(detail: &str) {
    local().send_replace(Local { status: "playing_singleplayer", detail: Some(detail.to_string()), host: None });
}

/// The running game opened its world to LAN on `port`.
pub fn host(port: u16, world: Option<String>, direct: Option<String>) {
    local().send_modify(|l| {
        l.status = "hosting_p2p";
        l.host = Some(Host { port, world, direct });
    });
}

/// The last running game exited.
pub fn in_menu() {
    local().send_replace(Local { status: "in_menu", detail: None, host: None });
}

/// Connects as the profile; does nothing when that profile is already connected.
pub fn connect(app: AppHandle, profile: ProfileRef) -> Result<(), String> {
    let uuid = profile.uuid()?;
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((current, task)) = running.as_ref() {
        if *current == uuid && !task.inner().is_finished() {
            return Ok(());
        }
    }
    if let Some((_, task)) = running.take() {
        task.abort();
    }
    let task = tauri::async_runtime::spawn(run(app, profile));
    *running = Some((uuid, task));
    Ok(())
}

pub fn disconnect(app: &AppHandle) {
    if let Some((_, task)) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).take() {
        task.abort();
    }
    state(app, json!({ "state": "off" }));
}

fn state(app: &AppHandle, value: Value) {
    let _ = app.emit("presence-state", value);
}

enum Ended {
    /// The server refused the token: sign in again.
    Unauthorized,
    /// The connection failed or dropped; `long` when it had been up for a while.
    Lost { error: String, long: bool },
}

async fn run(app: AppHandle, profile: ProfileRef) {
    let mut backoff = 1u64;
    let mut fresh = false;
    loop {
        state(&app, json!({ "state": "connecting" }));
        let token = match companion::token(&app, &profile, fresh).await {
            Ok(Some(t)) => Some(t),
            Ok(None) => {
                state(&app, json!({ "state": "unclaimed" }));
                return;
            }
            Err(e) => {
                state(&app, json!({ "state": "reconnecting", "retryIn": backoff, "error": e }));
                None
            }
        };
        if let Some(token) = token {
            match session(&app, &token).await {
                Ended::Unauthorized if !fresh => {
                    fresh = true;
                    continue;
                }
                Ended::Unauthorized => {
                    state(&app, json!({ "state": "reconnecting", "retryIn": backoff, "error": "Сервер Signal не принял вход" }));
                }
                Ended::Lost { error, long } => {
                    fresh = false;
                    if long {
                        backoff = 1;
                    }
                    state(&app, json!({ "state": "reconnecting", "retryIn": backoff, "error": error }));
                }
            }
        }
        // a little jitter, so launchers do not all come back in the same second after a server restart
        let jitter = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_millis() as u64);
        tokio::time::sleep(Duration::from_millis(backoff * 1000 + jitter)).await;
        backoff = (backoff * 2).min(BACKOFF_MAX_S);
    }
}

fn ws_url() -> String {
    let api = companion::api();
    let ws = api.strip_prefix("https://").map(|r| format!("wss://{r}")).or_else(|| api.strip_prefix("http://").map(|r| format!("ws://{r}")));
    format!("{}/presence/ws", ws.unwrap_or(api))
}

async fn session(app: &AppHandle, token: &str) -> Ended {
    let lost = |error: String| Ended::Lost { error, long: false };
    let mut req = match ws_url().into_client_request() {
        Ok(r) => r,
        Err(e) => return lost(format!("Неверный адрес сервера: {e}")),
    };
    match format!("Bearer {token}").parse() {
        Ok(v) => req.headers_mut().insert("authorization", v),
        Err(_) => return Ended::Unauthorized,
    };
    let mut ws = match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req)).await {
        Err(_) => return lost("Сервер Signal не ответил".into()),
        Ok(Err(tungstenite::Error::Http(res))) if res.status() == 401 => return Ended::Unauthorized,
        Ok(Err(tungstenite::Error::Http(res))) => return lost(format!("Сервер Signal ответил {}", res.status())),
        Ok(Err(e)) => return lost(format!("Сервер Signal недоступен: {e}")),
        Ok(Ok((ws, _))) => ws,
    };
    state(app, json!({ "state": "online" }));
    let since = Instant::now();
    let end = |error: &str| Ended::Lost { error: error.to_string(), long: since.elapsed() > Duration::from_secs(30) };

    let mut status = local().subscribe();
    status.mark_changed();
    let mut beat = tokio::time::interval(HEARTBEAT);
    beat.tick().await;
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            incoming = ws.next() => {
                let msg = match incoming {
                    None => return end("Сервер закрыл соединение"),
                    Some(Err(e)) => return end(&format!("Соединение прервалось: {e}")),
                    Some(Ok(m)) => m,
                };
                heard = Instant::now();
                match msg {
                    Message::Text(text) => {
                        let Ok(event) = serde_json::from_str::<Value>(&text) else { continue };
                        match event["type"].as_str() {
                            Some("pong") => {}
                            Some("relay_open") => relay_open(&event),
                            _ => {
                                let _ = app.emit("presence-event", event);
                            }
                        }
                    }
                    Message::Close(_) => return end("Сервер закрыл соединение"),
                    _ => {} // pings are answered by tungstenite on the next read or write
                }
            }
            changed = status.changed() => {
                if changed.is_err() {
                    return end("stopped");
                }
                let msg = status.borrow_and_update().message();
                if let Err(e) = ws.send(Message::text(msg)).await {
                    return end(&format!("Не удалось отправить статус: {e}"));
                }
            }
            _ = beat.tick() => {
                if heard.elapsed() > SILENT_LIMIT {
                    return end("Сервер не отвечает");
                }
                if let Err(e) = ws.send(Message::text(r#"{"type":"ping"}"#)).await {
                    return end(&format!("Соединение прервалось: {e}"));
                }
            }
        }
    }
}

/// A player reached the relay: open the data connection for them, to the Signal server only.
fn relay_open(event: &Value) {
    let (Some(id), Some(connect)) = (event["id"].as_str(), event["connect"].as_str()) else { return };
    let Some(port) = local().borrow().host.as_ref().map(|h| h.port) else { return };
    let api = companion::api();
    let server = api.split("://").nth(1).and_then(|r| r.split(['/', ':']).next()).unwrap_or_default();
    if connect.rsplit_once(':').map(|(h, _)| h) != Some(server) {
        println!("[signal] relay_open to a foreign host {connect} ignored");
        return;
    }
    tauri::async_runtime::spawn(crate::lanhost::pipe(connect.to_string(), id.to_string(), port));
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_or_world() {
        let l = super::Local { status: "playing_singleplayer", detail: Some("Fabric 1.21.1".into()), host: None };
        assert_eq!(l.message(), r#"{"detail":"Fabric 1.21.1","status":"playing_singleplayer","type":"status"}"#);
        let host = super::Host { port: 51234, world: Some("New World".into()), direct: None };
        let l = super::Local { status: "hosting_p2p", host: Some(host), ..l };
        assert_eq!(l.message(), r#"{"direct":null,"port":51234,"type":"host_lan","world":"New World"}"#);
    }

    #[test]
    fn socket_address_follows_the_api() {
        std::env::set_var("SIGNAL_SERVER", "http://127.0.0.1:3000/");
        assert_eq!(super::ws_url(), "ws://127.0.0.1:3000/v1/presence/ws");
        std::env::remove_var("SIGNAL_SERVER");
        assert_eq!(super::ws_url(), "wss://signallauncher.mooo.com/v1/presence/ws");
    }

    /// TLS works and the server answers the handshake (401 without a token once it serves presence).
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn live_handshake_reaches_the_server() {
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Error};
        let req = "wss://signallauncher.mooo.com/v1/presence/ws".into_client_request().unwrap();
        match tokio_tungstenite::connect_async(req).await {
            Err(Error::Http(res)) => println!("handshake answered {}", res.status()),
            other => panic!("expected an HTTP answer, got {:?}", other.map(|_| ())),
        }
    }
}
