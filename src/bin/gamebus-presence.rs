//! gamebus-presence — CLI for gamebus-presenced.
//!
//! Subcommands:
//! - `monitor` — pretty-prints the current bus state (activities, sources, names)
//! - `fetch-detectable` — downloads Discord's detectable.json to $XDG_CACHE_HOME

use zbus::Connection;

#[path = "../client.rs"]
mod client;

const DETECTABLE_URL: &str = "https://discord.com/api/v9/applications/detectable";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("monitor") => monitor().await,
        Some("fetch-detectable") => fetch_detectable(),
        Some("help") | Some("--help") | Some("-h") | None => {
            eprintln!("Usage: gamebus-presence <command>");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  monitor           Show current activity on the bus");
            eprintln!("  fetch-detectable  Download Discord's detectable.json naming database");
            eprintln!("  help              Show this help");
        }
        Some(cmd) => {
            eprintln!("Unknown command: {cmd}");
            eprintln!("Run 'gamebus-presence help' for usage.");
            std::process::exit(1);
        }
    }
}

/// Pretty-print the current bus state.
async fn monitor() {
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to connect to session bus: {e}");
            std::process::exit(1);
        }
    };

    let (version, has_activity, activities) = match client::snapshot(&conn).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to gamebus-presenced: {e}");
            eprintln!("Is the daemon running?");
            std::process::exit(1);
        }
    };

    println!("gamebus-presenced v{version}");
    println!("HasActivity: {has_activity}");
    println!();

    if activities.is_empty() {
        println!("No activities.");
        return;
    }

    for activity in &activities {
        println!("  {}", activity.path);
        println!("    Name:       {}", activity.display_name());
        println!("    Kind:       {}", activity.kind);
        println!("    Sources:    {}", activity.sources.join(", "));
        println!("    PID:        {}", activity.pid);
        if !activity.executable.is_empty() {
            println!("    Executable: {}", activity.executable);
        }
        if !activity.details.is_empty() {
            println!("    Details:    {}", activity.details);
        }
        if !activity.state.is_empty() {
            println!("    State:      {}", activity.state);
        }
        if let Some(elapsed) = activity.elapsed() {
            println!("    Playing:    {elapsed}");
        }
        if !activity.app_ids.is_empty() {
            let ids: Vec<String> = activity
                .app_ids
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            println!("    AppIds:     {}", ids.join(", "));
        }
        println!();
    }
}

/// Download Discord's detectable.json to the cache directory.
fn fetch_detectable() {
    let cache_dir = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .expect("no cache directory (set XDG_CACHE_HOME or HOME)");
    let dest_dir = cache_dir.join("gamebus-presenced");
    let dest = dest_dir.join("detectable.json");

    println!("Fetching {DETECTABLE_URL}...");
    let response = match ureq::get(DETECTABLE_URL).call() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Fetch failed: {e}");
            std::process::exit(1);
        }
    };

    std::fs::create_dir_all(&dest_dir).expect("failed to create cache directory");
    let mut body = response.into_reader();
    let mut file = std::fs::File::create(&dest).expect("failed to create detectable.json");
    std::io::copy(&mut body, &mut file).expect("failed to write detectable.json");

    let size = std::fs::metadata(&dest).unwrap().len();
    println!("Written {} bytes to {}", size, dest.display());
}
