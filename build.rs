//! Build script for gamebus-presenced.
//!
//! Fetches Discord's `applications/detectable` endpoint at build time
//! and writes it to the output directory as a data file. The packaging step
//! installs it alongside the binary (e.g. `$PREFIX/share/gamebus-presenced/`).
//!
//! If the fetch fails (no network, endpoint down), the build continues
//! without the file - the daemon degrades gracefully (no naming enrichment).

use std::path::Path;

const DETECTABLE_URL: &str = "https://discord.com/api/v9/applications/detectable";

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("detectable.json");

    // Only re-fetch if the file doesn't exist yet (cached across rebuilds).
    if dest.exists() {
        println!("cargo:rerun-if-changed=build.rs");
        return;
    }

    println!("cargo:warning=Fetching detectable.json from {DETECTABLE_URL}");
    match ureq::get(DETECTABLE_URL).call() {
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
}
