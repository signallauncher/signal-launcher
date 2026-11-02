//! «Открыть для сети» for friends (server/src/relay.rs): the world the game serves on LAN reaches them without
//! a white IP or router settings.
//!
//! The game logs `Started serving on <port>` when a world is opened to LAN. The launcher then asks the router
//! for a UPnP mapping and reports the world over the presence socket (`presence::host`). The server checks it
//! can reach the mapped address; if not (no UPnP, CGNAT, a firewall), it relays the world through its own
//! public port and asks for a data connection per player (`relay_open`), which `pipe` joins to the game.
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::time::Duration;

use igd_next::aio::tokio::{search_gateway, Tokio};
use igd_next::aio::Gateway;
use igd_next::{AddPortError, PortMappingProtocol, SearchOptions};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

const UPNP_TIMEOUT: Duration = Duration::from_secs(6);
/// Router leases are renewed by opening the world again; four hours covers a long evening.
const UPNP_LEASE_S: u32 = 4 * 3600;
const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// The level the integrated server loaded last (`Preparing level "…"`), for friends' lists.
static WORLD: Mutex<Option<String>> = Mutex::new(None);
/// The router mapping to remove when the game exits.
static MAPPING: Mutex<Option<(Gateway<Tokio>, u16)>> = Mutex::new(None);

/// One line of game output; cheap for everything that is not about the integrated server.
pub fn scan(line: &[u8]) {
    let has = |needle: &[u8]| line.windows(needle.len()).any(|w| w == needle);
    if !has(b"Preparing level") && !has(b"Started serving on") && !has(b"hosted on port") {
        return;
    }
    let text = String::from_utf8_lossy(line);
    if let Some(name) = text.split("Preparing level \"").nth(1).and_then(|r| r.split('"').next()) {
        *WORLD.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.to_string());
    }
    if let Some(port) = lan_port(&text) {
        let world = WORLD.lock().unwrap_or_else(|e| e.into_inner()).clone();
        tauri::async_runtime::spawn(async move {
            let direct = map_port(port).await;
            println!("[signal] world opened to LAN on {port}, UPnP: {}", direct.as_deref().unwrap_or("no"));
            crate::presence::host(port, world, direct);
        });
    }
}

/// `[Server thread/INFO]: Started serving on 51234`, or the chat line `Local game hosted on port 51234`.
fn lan_port(text: &str) -> Option<u16> {
    ["Started serving on ", "Local game hosted on port "].iter().find_map(|m| {
        let rest = text.split(m).nth(1)?;
        rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok().filter(|p| *p != 0)
    })
}

/// The game is gone: forget the world and give the router its port back.
pub fn game_exited() {
    *WORLD.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Some((gateway, port)) = MAPPING.lock().unwrap_or_else(|e| e.into_inner()).take() {
        tauri::async_runtime::spawn(async move {
            let _ = gateway.remove_port(PortMappingProtocol::TCP, port).await;
        });
    }
}

/// A UPnP mapping of the world's port: `public-ip:port`, or None (no router answer, a private or CGNAT
/// address outside, or the router refused).
async fn map_port(port: u16) -> Option<String> {
    tokio::time::timeout(UPNP_TIMEOUT, async {
        let mut options = SearchOptions::default();
        options.timeout = Some(Duration::from_secs(3));
        options.single_search_timeout = Some(Duration::from_secs(2));
        let gateway = search_gateway(options).await.ok()?;
        let external = gateway.get_external_ip().await.ok().filter(is_public)?;
        let local = SocketAddr::new(local_ip_towards(gateway.addr)?, port);
        let desc = "Signal Launcher: Minecraft LAN";
        let mapped = match gateway.add_port(PortMappingProtocol::TCP, port, local, UPNP_LEASE_S, desc).await {
            Ok(()) => port,
            Err(AddPortError::OnlyPermanentLeasesSupported) => {
                gateway.add_port(PortMappingProtocol::TCP, port, local, 0, desc).await.ok()?;
                port
            }
            Err(_) => gateway.add_any_port(PortMappingProtocol::TCP, local, UPNP_LEASE_S, desc).await.ok()?,
        };
        *MAPPING.lock().unwrap_or_else(|e| e.into_inner()) = Some((gateway, mapped));
        Some(SocketAddr::new(external, mapped).to_string())
    })
    .await
    .ok()
    .flatten()
}

/// This computer's address on the router's network.
fn local_ip_towards(gateway: SocketAddr) -> Option<IpAddr> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(gateway).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// Reachable from the internet: not private, CGNAT (100.64/10), loopback or link-local.
fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast()
                || v4.is_documentation()
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(_) => false,
    }
}

/// A relayed player: a data connection to the relay, joined to the game's LAN port.
pub async fn pipe(connect: String, id: String, port: u16) {
    let Ok(Ok(mut back)) = tokio::time::timeout(RELAY_CONNECT_TIMEOUT, TcpStream::connect(&connect)).await else {
        println!("[signal] relay {connect} unreachable");
        return;
    };
    if back.write_all(format!("SIGNAL-RELAY {id}\n").as_bytes()).await.is_err() {
        return;
    }
    let Ok(mut game) = TcpStream::connect(("127.0.0.1", port)).await else { return };
    let _ = back.set_nodelay(true);
    let _ = game.set_nodelay(true);
    let _ = tokio::io::copy_bidirectional(&mut back, &mut game).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_lan_port() {
        assert_eq!(lan_port("[12:00:01] [Server thread/INFO]: Started serving on 51234"), Some(51234));
        assert_eq!(lan_port("[12:00:01] [Render thread/INFO]: [System] [CHAT] Local game hosted on port 25566"), Some(25566));
        assert_eq!(lan_port("[12:00:01] [Server thread/INFO]: Started serving on "), None);
        assert_eq!(lan_port("Starting integrated minecraft server version 1.21.1"), None);
    }

    #[test]
    fn only_public_addresses_go_out() {
        for ip in ["192.168.1.5", "10.0.0.2", "100.72.3.4", "127.0.0.1", "169.254.1.1"] {
            assert!(!is_public(&ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public(&"87.120.187.118".parse().unwrap()));
    }
}
