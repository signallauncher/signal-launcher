//! Microsoft accounts: OAuth2 device code flow → Xbox Live → XSTS → Minecraft services.
//!
//! Live SDK endpoints with standard Client ID bypass Azure tenant verification and card requirements.
//! Accounts live in `<app data>/accounts.dat`, encrypted with Windows DPAPI.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::game;

// Live SDK endpoints (work without custom tenant / Azure registration)
const DEVICE_CODE_URL: &str = "https://login.live.com/oauth20_connect.srf";
const TOKEN_URL: &str = "https://login.live.com/oauth20_token.srf";
const REDIRECT_URI: &str = "https://login.live.com/oauth20_desktop.srf";
const SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";

const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// Game tokens are refreshed when less than this is left (they live 24 h).
const REFRESH_MARGIN_MS: i64 = 10 * 60 * 1000;

/// Default public Client ID
const BUILT_IN_CLIENT_ID: &str = "00000000402b5328";

pub fn client_id() -> Option<String> {
    std::env::var("SIGNAL_MS_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("SIGNAL_MS_CLIENT_ID").map(str::to_string))
        .or_else(|| Some(BUILT_IN_CLIENT_ID.to_string()))
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

// ---------------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
}

#[derive(Default)]
pub struct LoginState {
    pending: Mutex<Option<Pending>>,
    cancel: AtomicBool,
}

struct Pending {
    device_code: String,
    interval: u64,
    expires_at: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAccount {
    uuid: String,
    name: String,
    skin: Option<String>,
    slim: bool,
    face: Vec<String>,
    refresh_token: String,
    mc_token: String,
    mc_expires_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub id: String,
    pub name: String,
    pub skin: Option<String>,
    pub slim: bool,
    pub face: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub name: String,
    pub uuid: String,
    pub token: String,
}

impl From<&StoredAccount> for AccountInfo {
    fn from(a: &StoredAccount) -> Self {
        AccountInfo {
            id: a.uuid.clone(),
            name: a.name.clone(),
            skin: a.skin.clone(),
            slim: a.slim,
            face: a.face.clone(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Device code flow
// ---------------------------------------------------------------------------------------------

pub async fn start(state: &LoginState) -> Result<DeviceCode, String> {
    let client_id = client_id().unwrap_or_else(|| BUILT_IN_CLIENT_ID.to_string());
    let client = game::http_client()?;
    let data = post_form(
        &client,
        DEVICE_CODE_URL,
        &[
            ("client_id", client_id.as_str()),
            ("scope", SCOPE),
            ("response_type", "device_code"),
            ("redirect_uri", REDIRECT_URI),
        ],
    )
    .await?;

    if let Some(error) = data.get("error_description").or_else(|| data.get("error")).and_then(Value::as_str) {
        return Err(format!("Microsoft: {}", first_line(error)));
    }

    let field = |k: &str| data.get(k).and_then(Value::as_str).map(str::to_string);
    let device_code = field("device_code").ok_or("Microsoft не прислал код устройства")?;
    let expires_in = data.get("expires_in").and_then(Value::as_u64).unwrap_or(900);

    *state.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
        device_code,
        interval: data.get("interval").and_then(Value::as_u64).unwrap_or(5).max(1),
        expires_at: Instant::now() + Duration::from_secs(expires_in),
    });
    state.cancel.store(false, Ordering::SeqCst);

    Ok(DeviceCode {
        user_code: field("user_code").ok_or("Microsoft не прислал код для ввода")?,
        verification_uri: field("verification_uri").unwrap_or_else(|| "https://www.microsoft.com/link".into()),
        expires_in,
    })
}

pub fn cancel(state: &LoginState) {
    state.cancel.store(true, Ordering::SeqCst);
}

pub async fn finish(app: &AppHandle, state: &LoginState) -> Result<AccountInfo, String> {
    let client_id = client_id().unwrap_or_else(|| BUILT_IN_CLIENT_ID.to_string());
    let (device_code, mut interval, expires_at) = {
        let pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
        let p = pending.as_ref().ok_or("Сначала нужно получить код входа")?;
        (p.device_code.clone(), p.interval, p.expires_at)
    };
    let client = game::http_client()?;
    let tokens = loop {
        tokio_sleep(Duration::from_secs(interval)).await;
        if state.cancel.load(Ordering::SeqCst) {
            return Err("Вход отменён".into());
        }
        if Instant::now() >= expires_at {
            return Err("Код истёк — начните вход заново".into());
        }
        let data = post_form(
            &client,
            TOKEN_URL,
            &[
                ("client_id", client_id.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code.as_str()),
            ],
        )
        .await?;
        match data.get("error").and_then(Value::as_str) {
            None => break data,
            Some("authorization_pending") => continue,
            Some("slow_down") => interval += 5,
            Some("authorization_declined") => return Err("Вход отклонён в браузере".into()),
            Some("expired_token") | Some("bad_verification_code") | Some("invalid_grant") => {
                return Err("Код истёк или недействителен — начните вход заново".into())
            }
            Some(other) => {
                let text = data.get("error_description").and_then(Value::as_str).unwrap_or(other);
                return Err(format!("Microsoft: {}", first_line(text)));
            }
        }
    };
    state.pending.lock().unwrap_or_else(|e| e.into_inner()).take();

    let account = sign_in(&client, &tokens, None).await?;
    let info = AccountInfo::from(&account);
    let mut all = load(app);
    all.retain(|a| a.uuid != account.uuid);
    all.insert(0, account);
    save(app, &all)?;
    Ok(info)
}

async fn sign_in(client: &reqwest::Client, ms: &Value, previous: Option<&str>) -> Result<StoredAccount, String> {
    let raw_token = ms.get("access_token").and_then(Value::as_str).ok_or("Microsoft не выдал токен")?;
    let refresh_token = ms
        .get("refresh_token")
        .and_then(Value::as_str)
        .or(previous)
        .unwrap_or("")
        .to_string();

    let clean_token = raw_token.strip_prefix("d=").unwrap_or(raw_token);
    let rps_ticket = format!("d={clean_token}");

    // Xbox Live User Authenticate
    let xbl_body = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": rps_ticket
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT"
    });

    let (status, xbl_data) = post_json(client, XBL_URL, xbl_body)
        .await
        .map_err(|e| format!("Xbox Live: {e}"))?;

    if status != 200 {
        let alt_ticket = format!("t={clean_token}");
        let alt_body = json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": alt_ticket
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        });

        let (alt_status, alt_data) = post_json(client, XBL_URL, alt_body)
            .await
            .map_err(|e| format!("Xbox Live (retry): {e}"))?;

        if alt_status != 200 {
            let alt_data_str = if alt_data.is_null() || alt_data == "" {
                "пусто".to_string()
            } else {
                alt_data.to_string()
            };
            return Err(format!(
                "Xbox Live 401. Токен: {}... (len: {}). Ответ: {}",
                &clean_token[..clean_token.len().min(15)],
                clean_token.len(),
                alt_data_str
            ));
        }

        return complete_xsts_and_profile(client, alt_data, refresh_token).await;
    }

    complete_xsts_and_profile(client, xbl_data, refresh_token).await
}

async fn complete_xsts_and_profile(
    client: &reqwest::Client,
    xbl_data: Value,
    refresh_token: String,
) -> Result<StoredAccount, String> {
    let xbl_token = xbl_data
        .get("Token")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Xbox Live не выдал токен. Ответ: {xbl_data}"))?;

    let uhs = xbl_data
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(Value::as_str)
        .ok_or("Xbox Live не прислал userhash")?;

    // XSTS for Minecraft services
    let (status, xsts) = post_json(
        client,
        XSTS_URL,
        json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token]
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }),
    )
    .await?;

    if status == 401 {
        return Err(xsts_error(xsts.get("XErr").and_then(Value::as_u64).unwrap_or(0)));
    }
    if status != 200 {
        return Err(format!("XSTS отклонил запрос (HTTP {status}): {xsts}"));
    }

    let xsts_token = xsts.get("Token").and_then(Value::as_str).ok_or("XSTS не выдал токен")?;

    // Minecraft Login with Xbox
    let (status, mc) =
        post_json(client, MC_LOGIN_URL, json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") })).await?;

    if status == 403 {
        return Err("Minecraft отклонил вход (403 Forbidden)".into());
    }
    if status != 200 {
        return Err(format!("Minecraft отклонил вход (HTTP {status}): {mc}"));
    }

    let mc_token = mc
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Minecraft не выдал токен (HTTP {status})"))?
        .to_string();
    let expires_in = mc.get("expires_in").and_then(Value::as_i64).unwrap_or(86_400);

    // Profile & Skins
    let profile = client
        .get(PROFILE_URL)
        .bearer_auth(&mc_token)
        .send()
        .await
        .map_err(|e| format!("профиль Minecraft: {e}"))?;

    if profile.status().as_u16() == 404 {
        return Err("На этом аккаунте нет Minecraft: Java Edition".into());
    }
    let profile: Value = profile.json().await.map_err(|e| format!("профиль Minecraft: {e}"))?;
    let uuid = profile.get("id").and_then(Value::as_str).ok_or("в профиле нет UUID")?.to_string();
    let name = profile.get("name").and_then(Value::as_str).ok_or("в профиле нет никнейма")?.to_string();
    let active = profile
        .get("skins")
        .and_then(Value::as_array)
        .and_then(|skins| skins.iter().find(|s| s.get("state").and_then(Value::as_str) == Some("ACTIVE")));
    let slim = active.and_then(|s| s.get("variant")).and_then(Value::as_str) == Some("SLIM");
    let skin_png = match active.and_then(|s| s.get("url")).and_then(Value::as_str) {
        Some(url) => fetch(client, &url.replacen("http://", "https://", 1)).await.ok(),
        None => None,
    };
    let face = skin_png.as_deref().and_then(face_colors).unwrap_or_default();
    let skin = skin_png.map(|png| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)));

    Ok(StoredAccount {
        uuid,
        name,
        skin,
        slim,
        face,
        refresh_token,
        mc_token,
        mc_expires_at: game::now_ms() + expires_in * 1000,
    })
}

pub async fn session(app: &AppHandle, uuid: &str) -> Result<Session, String> {
    let mut all = load(app);
    let index = all
        .iter()
        .position(|a| a.uuid == uuid)
        .ok_or("Аккаунт Microsoft не найден — войдите заново")?;
    if all[index].mc_expires_at - game::now_ms() < REFRESH_MARGIN_MS {
        let client_id = client_id().unwrap_or_else(|| BUILT_IN_CLIENT_ID.to_string());
        let client = game::http_client()?;
        let old = all[index].refresh_token.clone();
        if !old.is_empty() {
            let tokens = post_form(
                &client,
                TOKEN_URL,
                &[
                    ("client_id", client_id.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", old.as_str()),
                ],
            )
            .await?;
            if tokens.get("error").is_none() {
                all[index] = sign_in(&client, &tokens, Some(&old)).await?;
                save(app, &all)?;
            }
        }
    }
    let a = &all[index];
    Ok(Session {
        name: a.name.clone(),
        uuid: a.uuid.clone(),
        token: a.mc_token.clone(),
    })
}

pub fn list(app: &AppHandle) -> Vec<AccountInfo> {
    load(app).iter().map(AccountInfo::from).collect()
}

pub fn remove(app: &AppHandle, uuid: &str) -> Result<(), String> {
    let mut all = load(app);
    all.retain(|a| a.uuid != uuid);
    save(app, &all)
}

fn xsts_error(code: u64) -> String {
    match code {
        2148916233 => "У этой учётной записи Microsoft нет профиля Xbox. Зайдите на xbox.com, создайте его и попробуйте снова.".into(),
        2148916235 => "Xbox Live недоступен в вашей стране.".into(),
        2148916236 | 2148916237 => "Нужна верификация возраста на xbox.com.".into(),
        2148916238 => "Это детский аккаунт: взрослый должен добавить его в семейную группу Microsoft.".into(),
        other => format!("XSTS отклонил вход (код {other})"),
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------------------------

async fn post_form(client: &reqwest::Client, url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let response = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("сеть недоступна: {e}"))?;
    response.json().await.map_err(|e| format!("некорректный ответ {url}: {e}"))
}

async fn post_json(client: &reqwest::Client, url: &str, body: Value) -> Result<(u16, Value), String> {
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("сеть недоступна: {e}"))?;

    let status = response.status().as_u16();
    let text = response.text().await.map_err(|e| e.to_string())?;
    let value = serde_json::from_str(&text).unwrap_or(Value::String(text));

    Ok((status, value))
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    Ok(response.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s).trim()
}

async fn tokio_sleep(d: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

// ---------------------------------------------------------------------------------------------
// Skin → 8×8 face
// ---------------------------------------------------------------------------------------------

fn face_colors(png_bytes: &[u8]) -> Option<Vec<String>> {
    let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        _ => return None,
    };
    if info.width < 64 || info.height < 32 {
        return None;
    }
    let pixel = |x: usize, y: usize| {
        let i = y * info.line_size + x * channels;
        let alpha = if channels == 4 { buf[i + 3] } else { 255 };
        (buf[i], buf[i + 1], buf[i + 2], alpha)
    };
    let mut out = Vec::with_capacity(64);
    for y in 8..16 {
        for x in 8..16 {
            let (mut r, mut g, mut b, _) = pixel(x, y);
            let (hr, hg, hb, ha) = pixel(x + 32, y);
            if ha > 0 {
                let a = ha as u32;
                r = ((hr as u32 * a + r as u32 * (255 - a)) / 255) as u8;
                g = ((hg as u32 * a + g as u32 * (255 - a)) / 255) as u8;
                b = ((hb as u32 * a + b as u32 * (255 - a)) / 255) as u8;
            }
            out.push(format!("#{r:02x}{g:02x}{b:02x}"));
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Storage (DPAPI on Windows)
// ---------------------------------------------------------------------------------------------

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|dir| dir.join("accounts.dat"))
}

fn load(app: &AppHandle) -> Vec<StoredAccount> {
    let Some(path) = store_path(app) else { return Vec::new() };
    let Ok(bytes) = fs::read(&path) else { return Vec::new() };
    unprotect(&bytes)
        .and_then(|plain| serde_json::from_slice(&plain).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, accounts: &[StoredAccount]) -> Result<(), String> {
    let path = store_path(app).ok_or("Нет папки данных приложения")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let plain = serde_json::to_vec(accounts).map_err(|e| e.to_string())?;
    let sealed = protect(&plain).ok_or("Не удалось зашифровать данные аккаунтов")?;
    fs::write(&path, sealed).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(windows)]
fn protect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, true)
}

#[cfg(windows)]
fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, false)
}

#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    unsafe {
        let ok = if encrypt {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        if ok == 0 || output.pbData.is_null() {
            return None;
        }
        let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        Some(out)
    }
}

#[cfg(not(windows))]
fn protect(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

#[cfg(not(windows))]
fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    Some(data.to_vec())
}