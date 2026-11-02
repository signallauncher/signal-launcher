//! Signal Companion API client (the Signal server, signal-api): proves the selected profile and syncs its look.
//!
//! Microsoft accounts prove themselves through Mojang's session server, the same way the game joins an
//! online server: the game token goes to Mojang only. Offline profiles get an Ed25519 key on first use.
//! Private keys and API tokens live in `<app data>/companion.dat`, encrypted with DPAPI like accounts.dat.
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::{auth, game};

const MOJANG_JOIN: &str = "https://sessionserver.mojang.com/session/minecraft/join";
/// A stored API token is renewed this long before it runs out.
const TOKEN_MARGIN_S: i64 = 3600;

/// `https://signallauncher.mooo.com/v1`, or the `SIGNAL_SERVER` override.
pub fn api() -> String {
    format!("{}/v1", crate::signal_server())
}

/// The profile picked in the launcher. Microsoft accounts carry their UUID in `id`.
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileRef {
    kind: String,
    id: Option<String>,
    name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Token {
    token: String,
    expires_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    /// offline UUID -> Ed25519 seed (base64url)
    #[serde(default)]
    keys: HashMap<String, String>,
    /// UUID -> API session
    #[serde(default)]
    tokens: HashMap<String, Token>,
}

/// Serialises read-modify-write of companion.dat between concurrent commands.
static STORE_LOCK: Mutex<()> = Mutex::new(());

fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map(|d| d.join("companion.dat")).map_err(|_| "Нет папки данных приложения".to_string())
}

fn load(app: &AppHandle) -> Store {
    let Ok(path) = store_path(app) else { return Store::default() };
    let Ok(bytes) = fs::read(path) else { return Store::default() };
    auth::unprotect(&bytes).and_then(|plain| serde_json::from_slice(&plain).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, store: &Store) -> Result<(), String> {
    let path = store_path(app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let plain = serde_json::to_vec(store).map_err(|e| e.to_string())?;
    let sealed = auth::protect(&plain).ok_or("Не удалось зашифровать ключи профиля")?;
    fs::write(&path, sealed).map_err(|e| format!("{}: {e}", path.display()))
}

fn update(app: &AppHandle, f: impl FnOnce(&mut Store)) -> Result<(), String> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(app);
    f(&mut store);
    save(app, &store)
}

fn now_s() -> i64 {
    game::now_ms() / 1000
}

fn dashed(hex: &str) -> String {
    let h: String = hex.chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
    if h.len() != 32 {
        return h;
    }
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// The UUID the game gives a nickname in offline mode (Java `UUID.nameUUIDFromBytes`).
pub fn offline_uuid(name: &str) -> String {
    let mut d = md5::compute(format!("OfflinePlayer:{name}")).0;
    d[6] = (d[6] & 0x0f) | 0x30;
    d[8] = (d[8] & 0x3f) | 0x80;
    dashed(&d.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

impl ProfileRef {
    pub fn uuid(&self) -> Result<String, String> {
        match self.kind.as_str() {
            "ms" => self.id.as_deref().map(dashed).ok_or_else(|| "У аккаунта Microsoft нет UUID".to_string()),
            "offline" if valid_name(&self.name) => Ok(offline_uuid(&self.name)),
            "offline" => Err("Ник: 3–16 символов, латиница, цифры и _".into()),
            _ => Err(format!("Неизвестный тип профиля: {}", self.kind)),
        }
    }
}

fn valid_name(name: &str) -> bool {
    (3..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

// ---------------------------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------------------------

/// Calls the API. Errors come back as (status, message) so callers can react to 401 or 409.
async fn call(client: &reqwest::Client, method: Method, path: &str, token: Option<&str>, body: Option<Value>) -> Result<Value, (StatusCode, String)> {
    let mut req = client.request(method, format!("{}{path}", api()));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, format!("Сервер Signal недоступен: {e}")))?;
    let status = res.status();
    let value: Value = res.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(value);
    }
    let message = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Сервер Signal ответил {status}"));
    Err((status, message))
}

// ---------------------------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------------------------

fn offline_key(app: &AppHandle, uuid: &str) -> Result<SigningKey, String> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(app);
    if let Some(seed) = store.keys.get(uuid).and_then(|s| URL_SAFE_NO_PAD.decode(s).ok()) {
        if let Ok(seed) = <[u8; 32]>::try_from(seed.as_slice()) {
            return Ok(SigningKey::from_bytes(&seed));
        }
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| format!("Нет источника случайных чисел: {e}"))?;
    store.keys.insert(uuid.to_string(), URL_SAFE_NO_PAD.encode(seed));
    save(app, &store)?;
    Ok(SigningKey::from_bytes(&seed))
}

async fn sign_in(app: &AppHandle, client: &reqwest::Client, p: &ProfileRef, uuid: &str) -> Result<Token, String> {
    let kind = if p.kind == "ms" { "mojang" } else { "offline" };
    let ch = call(client, Method::POST, "/auth/challenge", None, Some(json!({ "kind": kind, "uuid": uuid })))
        .await
        .map_err(|e| e.1)?;
    let challenge_id = ch["challengeId"].as_str().ok_or("Сервер Signal не выдал вызов")?;
    let reply = if p.kind == "ms" {
        let session = auth::session(app, uuid).await?;
        let server_id = ch["serverId"].as_str().ok_or("Сервер Signal не выдал serverId")?;
        let res = client
            .post(MOJANG_JOIN)
            .json(&json!({ "accessToken": session.token, "selectedProfile": uuid.replace('-', ""), "serverId": server_id }))
            .send()
            .await
            .map_err(|e| format!("Сессионный сервер Mojang недоступен: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("Mojang не принял вход ({}). Войдите в аккаунт Microsoft заново", res.status()));
        }
        call(client, Method::POST, "/auth/mojang", None, Some(json!({ "challengeId": challenge_id, "name": session.name })))
            .await
            .map_err(|e| e.1)?
    } else {
        let key = offline_key(app, uuid)?;
        let nonce = ch["nonce"].as_str().ok_or("Сервер Signal не выдал nonce")?;
        let message = format!("signal-companion/v1/offline-auth:{challenge_id}:{nonce}:{uuid}");
        let body = json!({
            "challengeId": challenge_id,
            "name": p.name,
            "publicKey": URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()),
            "signature": URL_SAFE_NO_PAD.encode(key.sign(message.as_bytes()).to_bytes()),
        });
        call(client, Method::POST, "/auth/offline", None, Some(body)).await.map_err(|(status, msg)| {
            if status == StatusCode::CONFLICT {
                format!("Ник «{}» на сервере Signal уже привязан к другому компьютеру", p.name)
            } else {
                msg
            }
        })?
    };
    let token = reply["token"].as_str().ok_or("Сервер Signal не выдал токен")?.to_string();
    let expires_at = reply["expiresAt"]
        .as_str()
        .and_then(|s| chrono_free_parse(s))
        .unwrap_or_else(|| now_s() + 7 * 24 * 3600);
    let t = Token { token, expires_at };
    let stored = t.clone();
    update(app, |s| {
        s.tokens.insert(uuid.to_string(), stored);
    })?;
    Ok(t)
}

/// `YYYY-MM-DDTHH:MM:SSZ` → unix seconds, without pulling in a date crate.
fn chrono_free_parse(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[19] != b'Z' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

fn stored_token(app: &AppHandle, uuid: &str) -> Option<Token> {
    load(app).tokens.get(uuid).cloned().filter(|t| t.expires_at - now_s() > TOKEN_MARGIN_S)
}

/// A bearer token for the profile (the presence socket sends it itself). `fresh` signs in again, after the
/// server refused the stored one. `None`: an offline nickname that was never claimed.
pub async fn token(app: &AppHandle, p: &ProfileRef, fresh: bool) -> Result<Option<String>, String> {
    let uuid = p.uuid()?;
    if p.kind == "offline" && !load(app).keys.contains_key(&uuid) {
        return Ok(None);
    }
    if !fresh {
        if let Some(t) = stored_token(app, &uuid) {
            return Ok(Some(t.token));
        }
    }
    let client = game::http_client()?;
    Ok(Some(sign_in(app, &client, p, &uuid).await?.token))
}

/// Calls the API as the profile: reuses the stored token, signs in when there is none or it was refused.
/// `claim: false` keeps an offline nickname that has no key yet unclaimed: the call is skipped (`Ok(None)`).
/// Nicknames are bound first come, first served, so only an explicit save may claim one (everybody starts as «Player»).
async fn as_profile(app: &AppHandle, p: &ProfileRef, claim: bool, method: Method, path: &str, body: Option<Value>) -> Result<Option<Value>, String> {
    let uuid = p.uuid()?;
    let store = load(app);
    if p.kind == "offline" && !claim && !store.keys.contains_key(&uuid) {
        return Ok(None);
    }
    let client = game::http_client()?;
    let token = match stored_token(app, &uuid) {
        Some(t) => t,
        None => sign_in(app, &client, p, &uuid).await?,
    };
    match call(&client, method.clone(), path, Some(&token.token), body.clone()).await {
        Err((StatusCode::UNAUTHORIZED, _)) => {
            let fresh = sign_in(app, &client, p, &uuid).await?;
            call(&client, method, path, Some(&fresh.token), body).await.map(Some).map_err(|e| e.1)
        }
        other => other.map(Some).map_err(|e| e.1),
    }
}

/// The profile's look on the server: `{ look, rev, updatedAt }` (rev 0: nothing saved yet),
/// or null for an offline nickname that was never saved to the server.
pub async fn look(app: &AppHandle, profile: ProfileRef) -> Result<Value, String> {
    Ok(match as_profile(app, &profile, false, Method::GET, "/me", None).await? {
        Some(me) => json!({ "look": me["look"], "rev": me["rev"], "updatedAt": me["updatedAt"] }),
        None => Value::Null,
    })
}

/// Saves the look for the profile: `{ rev, updatedAt }`.
pub async fn save_look(app: &AppHandle, profile: ProfileRef, look: Value) -> Result<Value, String> {
    Ok(as_profile(app, &profile, true, Method::PUT, "/me/look", Some(json!({ "look": look }))).await?.unwrap_or_default())
}

/// `{ friends, incoming, outgoing }` (the server's friends API), or `{ unclaimed: true }` for an offline
/// nickname that never talked to the server: it is claimed by its first friend request.
pub async fn friends(app: &AppHandle, profile: ProfileRef) -> Result<Value, String> {
    Ok(as_profile(app, &profile, false, Method::GET, "/friends", None).await?.unwrap_or_else(|| json!({ "unclaimed": true })))
}

/// Asks `name` to be friends: `{ state: "pending" | "friends", … }`.
pub async fn friend_request(app: &AppHandle, profile: ProfileRef, name: String) -> Result<Value, String> {
    let body = json!({ "name": name.trim() });
    Ok(as_profile(app, &profile, true, Method::POST, "/friends/request", Some(body)).await?.unwrap_or_default())
}

pub async fn friend_respond(app: &AppHandle, profile: ProfileRef, uuid: String, accept: bool) -> Result<Value, String> {
    let body = json!({ "uuid": uuid, "accept": accept });
    Ok(as_profile(app, &profile, false, Method::POST, "/friends/respond", Some(body)).await?.unwrap_or_default())
}

/// Removes a friend or withdraws a request.
pub async fn friend_remove(app: &AppHandle, profile: ProfileRef, uuid: String) -> Result<(), String> {
    as_profile(app, &profile, false, Method::POST, "/friends/remove", Some(json!({ "uuid": uuid }))).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_the_game() {
        assert_eq!(offline_uuid("Notch"), "b50ad385-829d-3141-a216-7e7d7539ba7f");
    }

    #[test]
    fn parses_api_timestamps() {
        assert_eq!(chrono_free_parse("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(chrono_free_parse("2026-10-13T12:00:30Z"), Some(1_791_892_830));
        assert_eq!(chrono_free_parse("garbage"), None);
    }
}
