//! Build script for gamebus-presenced.
//!
//! S4b: fetches Discord's `applications/detectable` endpoint at build time
//! and writes it to the output directory as a data file. The packaging step
//! installs it alongside the binary (e.g. `$PREFIX/share/gamebus-presenced/`).
//!
//! If the fetch fails (no network, endpoint down), the build continues
//! without the file — the daemon degrades gracefully (no naming enrichment).

use std::path::Path;

/// Fallback if endpoints.toml is unreadable — kept in step with it.
const DETECTABLE_URL: &str = "https://discord.com/api/v9/applications/detectable";

/// The URL comes from the shipped endpoints.toml (single source of truth;
/// the runtime tools read the same file via `src/endpoints.rs`).
fn detectable_url() -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let raw =
        std::fs::read_to_string(Path::new(&manifest).join("endpoints.toml")).unwrap_or_default();
    let mut in_discord = false;
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_discord = line == "[discord]";
        } else if in_discord {
            if let Some(value) = line
                .strip_prefix("detectable")
                .and_then(|l| l.trim_start().strip_prefix('='))
            {
                if let Some(url) = value
                    .trim()
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                {
                    return url.to_string();
                }
            }
        }
    }
    DETECTABLE_URL.to_string()
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("detectable.json");

    // Only re-fetch if the file doesn't exist yet (cached across rebuilds).
    if dest.exists() {
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-changed=endpoints.toml");
        return;
    }

    let detectable_url = detectable_url();
    println!("cargo:warning=Fetching detectable.json from {detectable_url}");
    match ureq::get(&detectable_url).call() {
        Ok(response) => {
            let mut body = response.into_reader();
            let mut file = std::fs::File::create(&dest).unwrap();
            std::io::copy(&mut body, &mut file).unwrap();
            let size = std::fs::metadata(&dest).unwrap().len();
            println!("cargo:warning=detectable.json: {} bytes written", size);
        }
        Err(e) => {
            println!(
                "cargo:warning=detectable.json fetch failed ({e}); building without naming data"
            );
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=endpoints.toml");
}
