//! The launcher's skin in an offline game without mods. An offline player has no skin URL, so the
//! game draws one of its default skins (picked by the player's UUID). A small resource pack
//! `resourcepacks/signal-skin/` replaces every default skin texture with the player's skin and is
//! switched on in `options.txt` before the launch.
//!
//! The UUID also picks the *model* (Classic or Slim), and changing the UUID would break offline
//! servers and the player's data in existing worlds, so the model stays the game's choice; the
//! frontend sends the skin in both arm widths and each texture gets the matching one.

use std::fs;
use std::io::Read;
use std::path::Path;

use base64::Engine;
use serde::Deserialize;
use serde_json::Value;

use crate::game;
use crate::packs;

pub const PACK: &str = "signal-skin";

/// Default skin names since 1.19.3 (`textures/entity/player/{slim,wide}/<name>.png`).
const DEFAULTS: [&str; 9] = ["alex", "ari", "efe", "kai", "makena", "noor", "steve", "sunny", "zuri"];

/// 64×64 PNGs (base64) of the same skin with Classic (4 px) and Slim (3 px) arms.
#[derive(Debug, Deserialize)]
pub struct SkinPayload {
    pub wide: String,
    pub slim: String,
}

/// Writes the pack and puts it on top of the installation's resource packs.
/// `jar` is the game's client jar: its `version.json` tells the resource pack format.
pub fn apply(instance: &Path, jar: &Path, mc: &str, skin: &SkinPayload) -> Result<(), String> {
    let decode = |b64: &str| -> Result<Vec<u8>, String> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| "Скин повреждён".to_string())?;
        // PNG signature + IHDR 64×64
        if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || bytes[16..24] != [0, 0, 0, 64, 0, 0, 0, 64] {
            return Err("Скин должен быть PNG 64×64".into());
        }
        if bytes.len() > 512 * 1024 {
            return Err("Файл скина слишком большой".into());
        }
        Ok(bytes)
    };
    let wide = decode(&skin.wide)?;
    let slim = decode(&skin.slim)?;

    let root = instance.join("resourcepacks").join(PACK);
    let _ = fs::remove_dir_all(&root);
    let textures = root.join("assets").join("minecraft").join("textures").join("entity");
    let write = |path: &Path, bytes: &[u8]| -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    };
    // 1.8 – 1.19.2
    write(&textures.join("steve.png"), &wide)?;
    write(&textures.join("alex.png"), &slim)?;
    // 1.19.3+
    for name in DEFAULTS {
        write(&textures.join("player").join("wide").join(format!("{name}.png")), &wide)?;
        write(&textures.join("player").join("slim").join(format!("{name}.png")), &slim)?;
    }
    write(&root.join("pack.mcmeta"), mcmeta(pack_format(jar, mc)).as_bytes())?;
    if let Some(icon) = head_icon(&wide) {
        write(&root.join("pack.png"), &icon)?;
    }

    enable(instance, mc)
}

/// Takes the pack out of `options.txt` and deletes it (Microsoft account, or the option is off).
pub fn remove(instance: &Path) {
    let root = instance.join("resourcepacks").join(PACK);
    if !root.exists() {
        return;
    }
    let key_new = format!("file/{PACK}");
    for key in ["resourcePacks", "incompatibleResourcePacks"] {
        let list = packs::option_list(instance, key);
        if list.iter().any(|p| p == &key_new || p == PACK) {
            let kept: Vec<String> = list.into_iter().filter(|p| p != &key_new && p != PACK).collect();
            let _ = packs::write_option(&instance.join("options.txt"), key, &serde_json::to_string(&kept).unwrap_or_else(|_| "[]".into()));
        }
    }
    let _ = fs::remove_dir_all(root);
}

/// Our pack last in `resourcePacks` (= highest priority). It is also listed as an accepted
/// incompatible pack, so a format the launcher guessed wrong never drops it.
fn enable(instance: &Path, mc: &str) -> Result<(), String> {
    // Before 1.13 entries are plain folder names and «vanilla» is implicit.
    let modern = game::version_key(mc) >= vec![1, 13] || !mc.starts_with("1.");
    let key = if modern { format!("file/{PACK}") } else { PACK.to_string() };
    let options = instance.join("options.txt");
    let mut list = packs::option_list(instance, "resourcePacks");
    list.retain(|p| p != &key);
    if modern && !list.iter().any(|p| p == "vanilla") {
        list.insert(0, "vanilla".into());
    }
    list.push(key.clone());
    packs::write_option(&options, "resourcePacks", &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()))?;
    if modern {
        let mut incompatible = packs::option_list(instance, "incompatibleResourcePacks");
        if !incompatible.contains(&key) {
            incompatible.push(key);
            packs::write_option(&options, "incompatibleResourcePacks", &serde_json::to_string(&incompatible).unwrap_or_else(|_| "[]".into()))?;
        }
    }
    Ok(())
}

/// Resource pack format of the client jar (`version.json → pack_version`), or a guess by version.
fn pack_format(jar: &Path, mc: &str) -> u32 {
    let from_jar = || -> Option<u32> {
        let mut archive = zip::ZipArchive::new(fs::File::open(jar).ok()?).ok()?;
        let mut text = String::new();
        archive.by_name("version.json").ok()?.read_to_string(&mut text).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        let pv = json.get("pack_version")?;
        pv.as_u64()
            .or_else(|| pv.get("resource").and_then(Value::as_u64))
            .or_else(|| pv.get("resource_major").and_then(Value::as_u64))
            .map(|v| v as u32)
    };
    from_jar().unwrap_or_else(|| {
        let v = game::version_key(mc);
        match v.as_slice() {
            [1, minor, ..] if *minor < 9 => 1,
            [1, minor, ..] if *minor < 11 => 2,
            [1, minor, ..] if *minor < 13 => 3,
            _ => 4,
        }
    })
}

/// Since format 65 (1.21.9) a pack declares `min_format`/`max_format`; older versions read `pack_format`.
fn mcmeta(format: u32) -> String {
    let description = "Скин из Signal Launcher";
    let pack = if format >= 65 {
        serde_json::json!({ "description": description, "min_format": format, "max_format": format })
    } else {
        serde_json::json!({ "description": description, "pack_format": format })
    };
    serde_json::to_string_pretty(&serde_json::json!({ "pack": pack })).unwrap_or_default()
}

/// The face (8×8 at 8,8 plus the hat layer) scaled ×8 as the pack's icon.
fn head_icon(skin: &[u8]) -> Option<Vec<u8>> {
    let decoder = png::Decoder::new(std::io::Cursor::new(skin));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight || info.width != 64 {
        return None;
    }
    let px = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * 64 + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    };
    let mut out = vec![0u8; 64 * 64 * 4];
    for y in 0..64 {
        for x in 0..64 {
            let (sx, sy) = (x / 8, y / 8);
            let base = px(8 + sx, 8 + sy);
            let hat = px(40 + sx, 8 + sy);
            let c = if hat[3] > 0 { hat } else { base };
            out[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 64, 64);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&out).ok()?;
    }
    Some(png_bytes)
}

/// Which model the game gives an offline nickname: since 1.19.3 one of 18 default skins
/// (`floorMod(uuid.hashCode(), 18)`, the first nine are Slim), before that Alex (Slim) when
/// `uuid.hashCode()` is odd; versions before 1.8 only know Classic.
pub fn offline_model(name: &str, mc: &str) -> &'static str {
    let uuid = game::offline_uuid(&game::sanitize_username(name));
    let bits = u128::from_str_radix(&uuid, 16).unwrap_or(0);
    let (msb, lsb) = ((bits >> 64) as u64, bits as u64);
    let hilo = msb ^ lsb;
    let hash = ((hilo >> 32) as i32) ^ (hilo as i32);
    let v = game::version_key(mc);
    let release = mc.starts_with("1.");
    if release && v < vec![1, 8] {
        return "wide";
    }
    let slim = if release && v < vec![1, 19, 3] { hash & 1 == 1 } else { hash.rem_euclid(18) < 9 };
    if slim { "slim" } else { "wide" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_meta_by_format() {
        assert!(mcmeta(34).contains("\"pack_format\": 34"));
        let modern = mcmeta(75);
        assert!(modern.contains("\"min_format\": 75") && !modern.contains("pack_format"));
    }

    #[test]
    fn model_follows_uuid_hash() {
        // Expected values from Java: UUID.nameUUIDFromBytes("OfflinePlayer:" + name).hashCode().
        for (name, modern, legacy) in [("Notch", "slim", "wide"), ("Player", "wide", "slim"), ("Steve_Pro", "slim", "slim")] {
            assert_eq!(offline_model(name, "1.21.1"), modern, "{name}");
            assert_eq!(offline_model(name, "26.3"), modern, "{name}");
            assert_eq!(offline_model(name, "1.12.2"), legacy, "{name}");
        }
        assert_eq!(offline_model("Player", "1.7.10"), "wide");
    }

    #[test]
    fn writes_pack_and_options() {
        let dir = std::env::temp_dir().join(format!("signal-skinpack-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("options.txt"), "version:3955\nresourcePacks:[\"vanilla\",\"file/a.zip\"]\n").unwrap();
        // 64×64 transparent PNG
        let mut png_bytes = Vec::new();
        {
            let mut e = png::Encoder::new(&mut png_bytes, 64, 64);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&vec![0u8; 64 * 64 * 4]).unwrap();
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let skin = SkinPayload { wide: b64.clone(), slim: b64 };
        apply(&dir, &dir.join("missing.jar"), "1.21.1", &skin).unwrap();
        let root = dir.join("resourcepacks").join(PACK);
        assert!(root.join("assets/minecraft/textures/entity/player/slim/zuri.png").is_file());
        assert!(root.join("assets/minecraft/textures/entity/steve.png").is_file());
        assert_eq!(packs::option_list(&dir, "resourcePacks"), vec!["vanilla", "file/a.zip", "file/signal-skin"]);
        assert_eq!(packs::option_list(&dir, "incompatibleResourcePacks"), vec!["file/signal-skin"]);
        // Applying twice keeps one entry; removing restores the list.
        apply(&dir, &dir.join("missing.jar"), "1.21.1", &skin).unwrap();
        assert_eq!(packs::option_list(&dir, "resourcePacks").len(), 3);
        remove(&dir);
        assert!(!root.exists());
        assert_eq!(packs::option_list(&dir, "resourcePacks"), vec!["vanilla", "file/a.zip"]);
        assert!(fs::read_to_string(dir.join("options.txt")).unwrap().contains("version:3955"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Formats from the real client jars: `cargo test --lib live_pack_format -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_pack_format() {
        let versions = game::minecraft_dir().join("versions");
        for (id, expected) in [("1.21.1", 34), ("1.21.11", 75), ("1.12.2", 3)] {
            let jar = versions.join(id).join(format!("{id}.jar"));
            if jar.is_file() {
                let got = pack_format(&jar, id);
                println!("{id}: {got}");
                assert_eq!(got, expected);
            }
        }
    }
}
