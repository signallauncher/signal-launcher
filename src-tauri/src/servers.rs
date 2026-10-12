//! Minecraft servers: Server List Ping (the request the game's server list makes) and the
//! servers players added in game (`servers.dat` of the shared folder and every installation).
//!
//! SLP (1.7+): handshake (next state = status) → status request → JSON status → ping/pong.
//! Addresses without a port also try the `_minecraft._tcp` SRV record, looked up over
//! DNS-over-HTTPS so no resolver library is needed.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::game;

const TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_PORT: u16 = 25565;
/// "-1": the client does not know the server version yet — the convention for pings.
const PROTOCOL_VERSION: i32 = -1;

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub online: bool,
    pub players_online: Option<i64>,
    pub players_max: Option<i64>,
    pub latency_ms: Option<u64>,
    /// Plain text, formatting codes removed, line breaks kept.
    pub motd: String,
    /// Version name as the server reports it ("Paper 1.21.1", "Requires MC 1.8 / 1.21").
    pub version: Option<String>,
    pub protocol: Option<i64>,
    /// `data:image/png;base64,…` server icon.
    pub favicon: Option<String>,
    /// Where it actually connected (after SRV).
    pub resolved: Option<String>,
    pub error: Option<String>,
}

/// One server from a `servers.dat`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedServer {
    pub name: String,
    pub address: String,
    pub icon: Option<String>,
    /// Where it was found: «Общая папка .minecraft» or the installation name.
    pub source: String,
}

/// `host[:port]` → status. Never fails: an unreachable server is `online: false` with a reason.
pub async fn ping(address: &str) -> ServerStatus {
    let address = address.trim().to_string();
    let Some((host, port)) = split_address(&address) else {
        return ServerStatus { error: Some("Некорректный адрес".into()), ..Default::default() };
    };
    // SRV only when no port is given, as the game does.
    let target = match port {
        Some(p) => (host.clone(), p),
        None => srv_lookup(&host).await.unwrap_or((host.clone(), DEFAULT_PORT)),
    };
    let handshake_host = host.clone();
    tauri::async_runtime::spawn_blocking(move || match status_blocking(&handshake_host, &target.0, target.1) {
        Ok(mut s) => {
            s.resolved = Some(format!("{}:{}", target.0, target.1));
            s
        }
        Err(e) => ServerStatus { error: Some(e), resolved: Some(format!("{}:{}", target.0, target.1)), ..Default::default() },
    })
    .await
    .unwrap_or_else(|e| ServerStatus { error: Some(e.to_string()), ..Default::default() })
}

fn status_blocking(handshake_host: &str, host: &str, port: u16) -> Result<ServerStatus, String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|_| "Адрес не найден (DNS)".to_string())?
        .next()
        .ok_or("Адрес не найден (DNS)")?;
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| connect_error(&e))?;
    let connect_ms = started.elapsed().as_millis() as u64;
    stream.set_read_timeout(Some(TIMEOUT)).ok();
    stream.set_write_timeout(Some(TIMEOUT)).ok();
    stream.set_nodelay(true).ok();

    // Handshake + status request in one write.
    let mut handshake = Vec::new();
    write_varint(&mut handshake, 0x00);
    write_varint(&mut handshake, PROTOCOL_VERSION);
    write_string(&mut handshake, handshake_host);
    handshake.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut handshake, 1);
    let mut out = frame(&handshake);
    out.extend(frame(&[0x00]));
    stream.write_all(&out).map_err(|e| format!("Сервер закрыл соединение: {e}"))?;

    let packet = read_packet(&mut stream)?;
    let mut cursor = &packet[..];
    if read_varint(&mut cursor)? != 0x00 {
        return Err("Сервер ответил не по протоколу Minecraft".into());
    }
    let len = read_varint(&mut cursor)? as usize;
    if len > cursor.len() {
        return Err("Ответ сервера обрезан".into());
    }
    let json: Value = serde_json::from_slice(&cursor[..len]).map_err(|e| format!("Сервер прислал непонятный статус: {e}"))?;

    // Ping/pong for the latency; some servers close right after the status — then the
    // connect time is the best estimate.
    let ping_started = Instant::now();
    let mut ping = Vec::new();
    write_varint(&mut ping, 0x01);
    ping.extend_from_slice(&(game::now_ms()).to_be_bytes());
    let latency = match stream.write_all(&frame(&ping)).ok().and_then(|_| read_packet(&mut stream).ok()) {
        Some(_) => ping_started.elapsed().as_millis() as u64,
        None => connect_ms,
    };

    Ok(ServerStatus {
        online: true,
        players_online: json.pointer("/players/online").and_then(Value::as_i64),
        players_max: json.pointer("/players/max").and_then(Value::as_i64),
        latency_ms: Some(latency.max(1)),
        motd: strip_codes(&chat_text(json.get("description").unwrap_or(&Value::Null))).trim().to_string(),
        version: json.pointer("/version/name").and_then(Value::as_str).map(|v| strip_codes(v).trim().to_string()),
        protocol: json.pointer("/version/protocol").and_then(Value::as_i64),
        favicon: json
            .get("favicon")
            .and_then(Value::as_str)
            .filter(|f| f.starts_with("data:image/png;base64,") && f.len() < 200_000)
            .map(|f| f.replace('\n', "")),
        resolved: None,
        error: None,
    })
}

fn connect_error(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "Нет ответа (таймаут 5 с)".into(),
        std::io::ErrorKind::ConnectionRefused => "Сервер выключен или порт закрыт".into(),
        _ => format!("Не удалось подключиться: {e}"),
    }
}

/// `host`, `host:port`, `[ipv6]:port`.
fn split_address(address: &str) -> Option<(String, Option<u16>)> {
    let a = address.trim();
    if a.is_empty() || a.contains(char::is_whitespace) || a.contains('/') {
        return None;
    }
    if let Some(rest) = a.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse().ok()?),
            None if tail.is_empty() => None,
            None => return None,
        };
        return Some((host.to_string(), port));
    }
    match a.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Some((host.to_string(), Some(port.parse().ok()?))),
        Some(_) => Some((a.to_string(), None)), // bare IPv6
        None => Some((a.to_string(), None)),
    }
}

/// `_minecraft._tcp.<host>` SRV over Cloudflare's DNS-over-HTTPS (JSON API).
async fn srv_lookup(host: &str) -> Option<(String, u16)> {
    if host.parse::<std::net::IpAddr>().is_ok() || host.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(4)).build().ok()?;
    let answer: Value = client
        .get("https://cloudflare-dns.com/dns-query")
        .query(&[("name", format!("_minecraft._tcp.{host}")), ("type", "SRV".into())])
        .header("Accept", "application/dns-json")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    // SRV data: "priority weight port target."
    answer.get("Answer")?.as_array()?.iter().find_map(|a| {
        let data = a.get("data")?.as_str()?;
        let parts: Vec<&str> = data.split_whitespace().collect();
        let port = parts.get(2)?.parse().ok()?;
        let target = parts.get(3)?.trim_end_matches('.').to_string();
        (!target.is_empty()).then_some((target, port))
    })
}

// ---------------------------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------------------------

fn write_varint(out: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7F == 0 {
            out.push(v as u8);
            return;
        }
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    write_varint(out, s.len() as i32);
    out.extend_from_slice(s.as_bytes());
}

/// Length-prefixed packet.
fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 3);
    write_varint(&mut out, body.len() as i32);
    out.extend_from_slice(body);
    out
}

fn read_varint(input: &mut &[u8]) -> Result<i32, String> {
    let mut value = 0u32;
    for i in 0..5 {
        let (&byte, rest) = input.split_first().ok_or("Ответ сервера обрезан")?;
        *input = rest;
        value |= ((byte & 0x7F) as u32) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(value as i32);
        }
    }
    Err("Слишком длинное число в ответе сервера".into())
}

fn read_varint_stream(stream: &mut impl Read) -> Result<i32, String> {
    let mut value = 0u32;
    for i in 0..5 {
        let mut b = [0u8; 1];
        stream.read_exact(&mut b).map_err(|e| read_error(&e))?;
        value |= ((b[0] & 0x7F) as u32) << (7 * i);
        if b[0] & 0x80 == 0 {
            return Ok(value as i32);
        }
    }
    Err("Слишком длинное число в ответе сервера".into())
}

fn read_packet(stream: &mut impl Read) -> Result<Vec<u8>, String> {
    let len = read_varint_stream(stream)?;
    if !(1..=1 << 21).contains(&len) {
        return Err("Сервер ответил не по протоколу Minecraft".into());
    }
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).map_err(|e| read_error(&e))?;
    Ok(buf)
}

fn read_error(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "Сервер не ответил на запрос статуса".into(),
        std::io::ErrorKind::UnexpectedEof => "Сервер закрыл соединение (возможно, это не сервер Minecraft)".into(),
        _ => format!("Ошибка чтения ответа: {e}"),
    }
}

/// Text of a chat component: string, `{text, extra}` or an array of them.
fn chat_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().map(chat_text).collect(),
        Value::Object(o) => {
            let mut s = o.get("text").map(chat_text).unwrap_or_default();
            if let Some(extra) = o.get("extra") {
                s.push_str(&chat_text(extra));
            }
            s
        }
        _ => String::new(),
    }
}

/// Removes `§x` formatting codes.
fn strip_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// servers.dat
// ---------------------------------------------------------------------------------------------

/// Servers saved in game, from the shared folder and every installation (first one wins).
pub fn saved() -> Vec<SavedServer> {
    let base = game::minecraft_dir();
    let mut roots = vec![(base.clone(), "Общая папка .minecraft".to_string())];
    for i in crate::instances::list() {
        roots.push((game::instance_dir(&i.id), i.name));
    }
    let mut out: Vec<SavedServer> = Vec::new();
    for (root, source) in roots {
        let Ok(bytes) = std::fs::read(root.join("servers.dat")) else { continue };
        let Ok(nbt) = fastnbt::from_bytes::<fastnbt::Value>(&bytes) else { continue };
        let fastnbt::Value::Compound(top) = nbt else { continue };
        let Some(fastnbt::Value::List(list)) = top.get("servers") else { continue };
        for entry in list {
            let fastnbt::Value::Compound(s) = entry else { continue };
            let text = |k: &str| match s.get(k) {
                Some(fastnbt::Value::String(v)) => Some(v.clone()),
                _ => None,
            };
            let Some(address) = text("ip").filter(|a| !a.trim().is_empty()) else { continue };
            if out.iter().any(|x| x.address.eq_ignore_ascii_case(&address)) {
                continue;
            }
            out.push(SavedServer {
                name: text("name").unwrap_or_else(|| address.clone()),
                icon: text("icon").map(|b64| format!("data:image/png;base64,{b64}")),
                address,
                source: source.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip() {
        for v in [0, 1, 127, 128, 255, 25565, 2_097_151, i32::MAX, -1] {
            let mut buf = Vec::new();
            write_varint(&mut buf, v);
            let mut slice = &buf[..];
            assert_eq!(read_varint(&mut slice).unwrap(), v);
            assert!(slice.is_empty());
        }
        let mut buf = Vec::new();
        write_varint(&mut buf, -1);
        assert_eq!(buf, [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[test]
    fn parses_addresses() {
        assert_eq!(split_address("mc.hypixel.net"), Some(("mc.hypixel.net".into(), None)));
        assert_eq!(split_address("play.example.org:25570"), Some(("play.example.org".into(), Some(25570))));
        assert_eq!(split_address("[::1]:25565"), Some(("::1".into(), Some(25565))));
        assert_eq!(split_address("192.168.1.20"), Some(("192.168.1.20".into(), None)));
        assert_eq!(split_address("host:notaport"), None);
        assert_eq!(split_address("bad host"), None);
        assert_eq!(split_address(""), None);
    }

    #[test]
    fn flattens_motd() {
        let v: Value = serde_json::json!({ "text": "§aHypixel §cNetwork", "extra": [{ "text": "\n§eSALE" }, "!"] });
        assert_eq!(strip_codes(&chat_text(&v)), "Hypixel Network\nSALE!");
        assert_eq!(chat_text(&Value::String("plain".into())), "plain");
    }

    /// Real servers: `cargo test --lib live_ping -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_ping() {
        for address in ["mc.hypixel.net", "play.cubecraft.net", "hypixel.net", "localhost:1"] {
            let s = tauri::async_runtime::block_on(ping(address));
            println!(
                "{address}: online={} {}/{} {}ms v={:?} via {:?} err={:?} motd={:?} icon={}",
                s.online,
                s.players_online.unwrap_or(-1),
                s.players_max.unwrap_or(-1),
                s.latency_ms.unwrap_or(0),
                s.version,
                s.resolved,
                s.error,
                s.motd.lines().next().unwrap_or(""),
                s.favicon.is_some()
            );
        }
    }
}
