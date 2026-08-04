//! gamebus-presence — CLI for gamebus-presenced.
//!
//! Subcommands:
//! - `monitor` — pretty-prints the current bus state (activities, sources, names)
//! - `fetch-detectable` — downloads Discord's detectable.json to $XDG_CACHE_HOME

use std::collections::HashMap;
use zbus::zvariant::OwnedObjectPath;
use zbus::{proxy, Connection};

const DETECTABLE_URL: &str = "https://discord.com/api/v9/applications/detectable";

/// Client proxy for the Manager interface.
#[proxy(
    interface = "org.gamebus.Presence.v1.Manager",
    default_service = "org.gamebus.Presence.v1",
    default_path = "/org/gamebus/Presence/v1"
)]
trait Manager {
    fn list_activities(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    #[zbus(property)]
    fn has_activity(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn version(&self) -> zbus::Result<u64>;
}

/// Client proxy for Activity objects.
#[proxy(
    interface = "org.gamebus.Presence.v1.Activity",
    default_service = "org.gamebus.Presence.v1",
    assume_defaults = false
)]
trait ActivityProps {
    #[zbus(property)]
    fn sources(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn kind(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn details(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn process_id(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn executable(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn app_ids(&self) -> zbus::Result<HashMap<String, String>>;
    #[zbus(property)]
    fn since(&self) -> zbus::Result<u64>;
}

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

    let manager = match ManagerProxy::new(&conn).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to connect to gamebus-presenced: {e}");
            eprintln!("Is the daemon running?");
            std::process::exit(1);
        }
    };

    let has_activity = manager.has_activity().await.unwrap_or(false);
    let version = manager.version().await.unwrap_or(0);
    println!("gamebus-presenced v{version}");
    println!("HasActivity: {has_activity}");
    println!();

    let activities = manager.list_activities().await.unwrap_or_default();
    if activities.is_empty() {
        println!("No activities.");
        return;
    }

    for path in &activities {
        let activity = match ActivityPropsProxy::builder(&conn)
            .path(path.as_str())
            .unwrap()
            .build()
            .await
        {
            Ok(a) => a,
            Err(e) => {
                eprintln!("  {}: error reading properties: {e}", path.as_str());
                continue;
            }
        };

        let name = activity.name().await.unwrap_or_default();
        let kind = activity.kind().await.unwrap_or_default();
        let sources = activity.sources().await.unwrap_or_default();
        let pid = activity.process_id().await.unwrap_or(0);
        let details = activity.details().await.unwrap_or_default();
        let state = activity.state().await.unwrap_or_default();
        let executable = activity.executable().await.unwrap_or_default();
        let app_ids = activity.app_ids().await.unwrap_or_default();

        println!("  {}", path.as_str());
        println!(
            "    Name:       {}",
            if name.is_empty() { "(unknown)" } else { &name }
        );
        println!("    Kind:       {kind}");
        println!("    Sources:    {}", sources.join(", "));
        println!("    PID:        {pid}");
        if !executable.is_empty() {
            println!("    Executable: {executable}");
        }
        if !details.is_empty() {
            println!("    Details:    {details}");
        }
        if !state.is_empty() {
            println!("    State:      {state}");
        }
        if !app_ids.is_empty() {
            let ids: Vec<String> = app_ids.iter().map(|(k, v)| format!("{k}={v}")).collect();
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
