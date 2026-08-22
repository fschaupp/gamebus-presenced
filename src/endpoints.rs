//! The configurable network endpoints (see `endpoints.toml` at the repo
//! root - the bundled copy of that file is the last-resort default).
//!
//! Compiled into the CLI tools only. The daemon is network-free and has no
//! business knowing a URL; nothing here is reachable from `src/main.rs`.
//!
//! The parser handles exactly the subset the shipped file uses — `[section]`
//! headers, `key = "value"` lines, `#` comments — by hand, like the CSV and
//! date code elsewhere: a TOML crate would be a dependency for a handful of
//! keys.

use std::collections::HashMap;
use std::path::PathBuf;

/// The shipped defaults, compiled in. A unit test proves this parses and
/// carries every key, so the `expect`s in [`Endpoints::load`] cannot fire.
const BUNDLED: &str = include_str!("../endpoints.toml");

pub const ENDPOINTS_NAME: &str = "endpoints.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Discord's public detectable-applications endpoint.
    pub discord_detectable: String,
    /// The umu-database query API (bare endpoint = full dump).
    pub umu_api: String,
    /// The umu-database repository, for submission hints.
    pub umu_repository: String,
    /// Its open merge requests (GitHub API), for `--check-prs`.
    pub umu_open_prs: String,
    /// The protonfixes repository's file list (GitHub API, one request):
    /// which games need umu at all, and so which entries the database wants.
    pub umu_protonfixes_tree: String,
    /// One protonfix file's page, for the merge request's evidence lines.
    pub umu_protonfixes_file: String,
    /// GOG's public catalog search (the misses pane's `o` lookup).
    pub gog_catalog: String,
    /// GOG's products API root: `<product>/<id>` answers with the store's
    /// own title — the `o` reverse lookup for numeric gog codenames.
    pub gog_product: String,
    /// gogdb.org product pages — the GOG codename authority, linked per
    /// product id in the exported evidence.
    pub gog_gogdb_product: String,
    /// egdata.app's offer search (the `o` lookup for egs entries).
    pub egs_search: String,
    /// egdata.app's sandboxes root: `<sandboxes>/<namespace>/builds`.
    pub egs_sandboxes: String,
}

impl Endpoints {
    /// The effective endpoints: the first config file found (user config
    /// dir, then the data dirs) layered over the bundled defaults - a
    /// partial file overrides only what it names. No file, or an unreadable
    /// one, means the defaults; endpoints must never be a reason the tools
    /// cannot start.
    pub fn load() -> Self {
        let mut values = parse(BUNDLED);
        if let Some(raw) = candidate_paths()
            .into_iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
        {
            for (key, value) in parse(&raw) {
                values.insert(key, value);
            }
        }
        let get = |key: &str| {
            values
                .get(key)
                .cloned()
                .unwrap_or_else(|| panic!("bundled endpoints.toml is missing {key}"))
        };
        Self {
            discord_detectable: get("discord.detectable"),
            umu_api: get("umu.api"),
            umu_repository: get("umu.repository"),
            umu_open_prs: get("umu.open_prs"),
            umu_protonfixes_tree: get("umu.protonfixes_tree"),
            umu_protonfixes_file: get("umu.protonfixes_file"),
            gog_catalog: get("gog.catalog"),
            gog_product: get("gog.product"),
            gog_gogdb_product: get("gog.gogdb_product"),
            egs_search: get("egs.search"),
            egs_sandboxes: get("egs.sandboxes"),
        }
    }
}

/// The search order documented in the shipped file: user config first, then
/// installed copies.
fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    {
        paths.push(config.join("gamebus-presenced").join(ENDPOINTS_NAME));
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    {
        paths.push(data.join("gamebus-presenced").join(ENDPOINTS_NAME));
    }
    for dir in ["/usr/local/share", "/usr/share"] {
        paths.push(
            PathBuf::from(dir)
                .join("gamebus-presenced")
                .join(ENDPOINTS_NAME),
        );
    }
    paths
}

/// `section.key → value` for the TOML subset the file uses. Unknown lines
/// are skipped, not errors: a future key must not break an old binary.
fn parse(raw: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut section = String::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            // Values are quoted strings; anything else is not ours.
            if let Some(value) = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .filter(|v| !v.is_empty())
            {
                values.insert(format!("{section}.{key}"), value.to_string());
            }
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_file_parses_and_carries_every_key() {
        let values = parse(BUNDLED);
        for key in [
            "discord.detectable",
            "umu.api",
            "umu.repository",
            "umu.open_prs",
            "umu.protonfixes_tree",
            "umu.protonfixes_file",
            "gog.catalog",
            "gog.product",
            "gog.gogdb_product",
            "egs.search",
            "egs.sandboxes",
        ] {
            assert!(
                values.get(key).is_some_and(|v| v.starts_with("https://")),
                "bundled endpoints.toml missing or malformed: {key}"
            );
        }
    }

    #[test]
    fn a_partial_override_keeps_the_other_defaults() {
        let mut values = parse(BUNDLED);
        for (k, v) in parse("[umu]\napi = \"http://localhost:9\"\n") {
            values.insert(k, v);
        }
        assert_eq!(values["umu.api"], "http://localhost:9");
        assert!(values["discord.detectable"].contains("discord.com"));
        assert!(values["gog.catalog"].contains("catalog.gog.com"));
        assert!(values["gog.product"].contains("api.gog.com"));
        assert!(values["egs.sandboxes"].contains("egdata.app"));
    }

    #[test]
    fn garbage_lines_are_skipped_not_fatal() {
        let values = parse("nonsense\n[umu]\napi = unquoted\nempty = \"\"\napi = \"http://x\"\n");
        assert_eq!(values.len(), 1);
        assert_eq!(values["umu.api"], "http://x");
    }
}
