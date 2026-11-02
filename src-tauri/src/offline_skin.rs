//! Offline accounts: the skin that goes with a nickname. The Minecraft account with that name
//! (Mojang API → session profile → textures), else Ely.by's skin system; nothing found → the
//! launcher's default skin. The PNG comes back as a data URL plus the 8×8 face for the avatar.
//! Only `textures.minecraft.net` and `ely.by` are fetched from.

use std::io::Cursor;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use futures_util::future::Either;
use serde::Serialize;
use serde_json::Value;

/// Name → UUID (the Minecraft Services lookup answers a little faster and steadier than api.mojang.com).
const MOJANG_NAME_URL: &str = "https://api.minecraftservices.com/minecraft/profile/lookup/name";
const SESSION_PROFILE_URL: &str = "https://sessionserver.mojang.com/session/minecraft/profile";
const ELYBY_TEXTURES_URL: &str = "https://skinsystem.ely.by/textures";
/// Per request (name, profile, textures, PNG): a lookup must not stall the nickname field.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
/// A 64×64 skin is a few KB; anything this big is not a skin.
const MAX_SKIN_BYTES: usize = 256 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineSkin {
    /// `mojang` or `elyby`.
    pub source: String,
    /// `data:image/png;base64,…`
    pub skin: String,
    pub slim: bool,
    /// 8×8 face (hat layer included), like Microsoft accounts.
    pub face: Vec<String>,
}

/// Skin for an offline nickname: the Minecraft account with that name (Mojang), else Ely.by.
/// `Ok(None)` — both answered that the name has no skin (the launcher keeps its default skin and caches
/// that); a source that did not answer makes it an error instead, so the launcher asks again later.
pub async fn resolve_offline_skin(nickname: &str) -> Result<Option<OfflineSkin>, String> {
    let name = nickname.trim();
    if name.len() < 3 || name.len() > 16 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("Никнейм: 3–16 символов, латиница, цифры и _".into());
    }
    let client = client()?;
    // Both sources start at once. Mojang wins: its answer is awaited even when Ely.by is first, but a skin
    // from Mojang is returned without waiting for Ely.by.
    // Mojang's chain is three requests long and its name lookup has slow moments: one retry.
    let mojang = async {
        match mojang_full(&client, name).await {
            Err(_) => mojang_full(&client, name).await,
            answer => answer,
        }
    };
    let elyby = async {
        match elyby_skin(&client, name).await? {
            Some((url, slim)) => skin_from(&client, &url, slim, "elyby").await.map(Some),
            None => Ok(None),
        }
    };
    futures_util::pin_mut!(mojang, elyby);
    let (mojang, elyby) = match futures_util::future::select(mojang, elyby).await {
        Either::Left((Ok(Some(skin)), _)) => return Ok(Some(skin)),
        Either::Left((mojang, elyby)) => (mojang, elyby.await),
        Either::Right((elyby, mojang)) => (mojang.await, elyby),
    };
    match (mojang, elyby) {
        (Ok(Some(skin)), _) | (_, Ok(Some(skin))) => Ok(Some(skin)),
        (Ok(None), Ok(None)) => Ok(None),
        (Err(e), _) | (_, Err(e)) => Err(format!("Скин не получен: {e}")),
    }
}

async fn mojang_full(client: &reqwest::Client, name: &str) -> Result<Option<OfflineSkin>, String> {
    match mojang_skin(client, name).await? {
        Some((url, slim)) => skin_from(client, &url, slim, "mojang").await.map(Some),
        None => Ok(None),
    }
}

/// Opens the connections (TLS included) a lookup needs while the nickname is still being typed, so the
/// first lookup is as quick as the next ones. Answers are ignored.
pub async fn warm_up() {
    let Ok(client) = client() else { return };
    let hosts = ["https://api.minecraftservices.com/", "https://sessionserver.mojang.com/", "https://textures.minecraft.net/", "https://skinsystem.ely.by/"];
    futures_util::future::join_all(hosts.iter().map(|url| client.head(*url).send())).await;
}

/// One client for every lookup, so typing a nickname reuses the open connections (no TLS handshake per
/// letter). Each request gets REQUEST_TIMEOUT: a slow or dead source must not hold the skin back.
fn client() -> Result<reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("SignalLauncher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| client).clone())
}

/// Name → UUID → textures property of the session profile → (skin url, slim).
async fn mojang_skin(client: &reqwest::Client, name: &str) -> Result<Option<(String, bool)>, String> {
    let response = client.get(format!("{MOJANG_NAME_URL}/{name}")).send().await.map_err(|e| format!("Mojang: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        204 | 404 => return Ok(None),
        code => return Err(format!("Mojang: HTTP {code}")),
    }
    let user: Value = response.json().await.map_err(|e| format!("Mojang: {e}"))?;
    let Some(uuid) = user.get("id").and_then(Value::as_str).filter(|id| id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())) else {
        return Ok(None);
    };
    let profile: Value = client
        .get(format!("{SESSION_PROFILE_URL}/{uuid}"))
        .send()
        .await
        .map_err(|e| format!("Mojang: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Mojang: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Mojang: {e}"))?;
    let encoded = profile
        .get("properties")
        .and_then(Value::as_array)
        .and_then(|p| p.iter().find(|x| x.get("name").and_then(Value::as_str) == Some("textures")))
        .and_then(|x| x.get("value").and_then(Value::as_str));
    let Some(encoded) = encoded else { return Ok(None) };
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|e| format!("Mojang: {e}"))?;
    let textures: Value = serde_json::from_slice(&decoded).map_err(|e| format!("Mojang: {e}"))?;
    Ok(skin_entry(textures.pointer("/textures/SKIN")))
}

/// Ely.by answers `{"SKIN":{"url":…,"metadata":{"model":"slim"}}}`, or 204 for an unknown name.
async fn elyby_skin(client: &reqwest::Client, name: &str) -> Result<Option<(String, bool)>, String> {
    let response = client.get(format!("{ELYBY_TEXTURES_URL}/{name}")).send().await.map_err(|e| format!("Ely.by: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        204 | 404 => return Ok(None),
        code => return Err(format!("Ely.by: HTTP {code}")),
    }
    let textures: Value = response.json().await.map_err(|e| format!("Ely.by: {e}"))?;
    Ok(skin_entry(textures.get("SKIN")))
}

/// `{"url": …, "metadata": {"model": "slim"}}` → (https url on an allowed host, slim).
fn skin_entry(skin: Option<&Value>) -> Option<(String, bool)> {
    let skin = skin?;
    let url = skin.get("url")?.as_str()?.replacen("http://", "https://", 1);
    let host = url.strip_prefix("https://")?.split(['/', '?', '#']).next().unwrap_or("");
    if !["textures.minecraft.net", "ely.by", "skinsystem.ely.by"].contains(&host) {
        return None;
    }
    let slim = skin.pointer("/metadata/model").and_then(Value::as_str) == Some("slim");
    Some((url, slim))
}

async fn skin_from(client: &reqwest::Client, url: &str, slim: bool, source: &str) -> Result<OfflineSkin, String> {
    let png = fetch(client, url).await.map_err(|e| format!("скин не скачался: {e}"))?;
    if png.len() > MAX_SKIN_BYTES || png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("по ссылке не PNG-скин".into());
    }
    // IHDR: width 64, height 64 or 32 (the launcher's skin tools work with these).
    let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    if w != 64 || (h != 64 && h != 32) {
        return Err(format!("скин {w}×{h}, а нужен 64×64"));
    }
    let face = face_colors(&png).unwrap_or_default();
    Ok(OfflineSkin {
        source: source.into(),
        skin: format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png)),
        slim,
        face,
    })
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    Ok(response.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

/// The 8×8 face with the hat layer over it, as `#rrggbb` colors.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_entries_keep_to_known_hosts() {
        let mojang = serde_json::json!({ "url": "http://textures.minecraft.net/texture/abc", "metadata": { "model": "slim" } });
        assert_eq!(skin_entry(Some(&mojang)), Some(("https://textures.minecraft.net/texture/abc".to_string(), true)));
        let ely = serde_json::json!({ "url": "http://ely.by/storage/skins/x.png" });
        assert_eq!(skin_entry(Some(&ely)), Some(("https://ely.by/storage/skins/x.png".to_string(), false)));
        assert_eq!(skin_entry(Some(&serde_json::json!({ "url": "https://evil.example/x.png" }))), None);
        assert_eq!(skin_entry(None), None);
    }

    #[test]
    fn rejects_bad_nicknames() {
        assert!(tauri::async_runtime::block_on(resolve_offline_skin("a")).is_err());
        assert!(tauri::async_runtime::block_on(resolve_offline_skin("bad name!")).is_err());
    }

    /// `cargo test --lib live_offline_skin -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_offline_skin() {
        let started = std::time::Instant::now();
        tauri::async_runtime::block_on(warm_up());
        println!("warm-up: {} ms", started.elapsed().as_millis());
        for name in ["Notch", "ErickSkrauch", "zz_no_such_x9", "Notch"] {
            let started = std::time::Instant::now();
            let r = tauri::async_runtime::block_on(resolve_offline_skin(name)).unwrap();
            let ms = started.elapsed().as_millis();
            println!("{name}: {ms} ms {:?}", r.as_ref().map(|s| (s.source.clone(), s.slim, s.face.len(), s.skin.len())));
        }
    }
}
