fn main() {
    // CurseForge API key of the launcher's developer, baked into the build so players need no
    // setup: `SIGNAL_CF_API_KEY` from the environment, or the first line of `curseforge-api.key`
    // next to this file (ignored by git, like every *.key). Without a key the CurseForge source is
    // simply not shown in the catalog.
    println!("cargo:rerun-if-env-changed=SIGNAL_CF_API_KEY");
    println!("cargo:rerun-if-changed=curseforge-api.key");
    if std::env::var("SIGNAL_CF_API_KEY").map(|k| k.trim().is_empty()).unwrap_or(true) {
        if let Ok(text) = std::fs::read_to_string("curseforge-api.key") {
            let key = text.lines().next().unwrap_or("").trim();
            if !key.is_empty() {
                println!("cargo:rustc-env=SIGNAL_CF_API_KEY={key}");
            }
        }
    }
    tauri_build::build()
}
