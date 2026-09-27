//! The protonfix list: which games actually need umu.
//!
//! The umu-database opens with its own scope rule - "We focus on games that
//! requires fixes in Proton. Games that run out of the box have no need be
//! added to the database." A store copy earns a row when a protonfix exists
//! for its umu id and nothing maps that copy to it yet; without a fix, a
//! submission is work for the maintainers and nothing for the player.
//!
//! Upstream keeps one file per fix, named by the id it serves:
//! `gamefixes-steam/870780.py` for Steam appids, `gamefixes-<store>/umu-<id>.py`
//! for everyone else (most of those are symlinks to the Steam fix - exactly
//! the "link the other store's copy to the same fix" mechanism a database
//! row unlocks). One request for the repository's file list is the whole
//! check; the list is cached like the database dump and refreshed on the
//! user's word.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::{endpoints, HTTP_TIMEOUT, USER_AGENT};

/// The upstream fix files, indexed by the umu id they serve.
#[derive(Debug, Default)]
pub(super) struct ProtonFixes {
    by_umu_id: HashMap<String, Vec<String>>,
}

impl ProtonFixes {
    /// Parse GitHub's recursive tree listing - the shape one request to
    /// `.../git/trees/<branch>?recursive=1` answers with.
    pub(super) fn parse_tree(raw: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Tree {
            tree: Vec<Node>,
        }
        #[derive(Deserialize)]
        struct Node {
            path: String,
        }
        let tree: Tree =
            serde_json::from_str(raw).map_err(|e| format!("not a GitHub tree listing: {e}"))?;
        Self::from_paths(tree.tree.into_iter().map(|n| n.path))
    }

    /// Parse the cache: the fix paths, one per line.
    pub(super) fn parse_cache(raw: &str) -> Result<Self, String> {
        Self::from_paths(raw.lines().map(str::to_string))
    }

    fn from_paths(paths: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut by_umu_id: HashMap<String, Vec<String>> = HashMap::new();
        for path in paths {
            let Some(id) = umu_id_of(&path) else { continue };
            by_umu_id.entry(id).or_default().push(path);
        }
        if by_umu_id.is_empty() {
            return Err("no protonfix files in the listing".into());
        }
        for paths in by_umu_id.values_mut() {
            paths.sort();
        }
        Ok(Self { by_umu_id })
    }

    /// The fix files serving this umu id, or an empty slice: the game runs
    /// without umu's help and the database does not want it.
    pub(super) fn fixes_for(&self, umu_id: &str) -> &[String] {
        self.by_umu_id
            .get(&umu_id.to_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(super) fn len(&self) -> usize {
        self.by_umu_id.len()
    }

    fn cache_text(&self) -> String {
        let mut paths: Vec<&String> = self.by_umu_id.values().flatten().collect();
        paths.sort();
        paths
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Which umu id a repository path serves, if any. Steam fixes are named by
/// the bare appid, every other store by the full umu id; `__init__.py` and
/// the per-store `default.py` serve no single game.
fn umu_id_of(path: &str) -> Option<String> {
    let (dir, file) = path.split_once('/')?;
    let store = dir.strip_prefix("gamefixes-")?;
    let name = file.strip_suffix(".py")?;
    if name.is_empty() || name == "__init__" || name == "default" || file.contains('/') {
        return None;
    }
    let id = if name.starts_with("umu-") {
        name.to_string()
    } else if store == "steam" {
        format!("umu-{name}")
    } else {
        // A bare name outside the Steam directory names no id we can match.
        return None;
    };
    Some(id.to_lowercase())
}

/// Where the fix list is cached, beside the database dump.
pub(super) fn cache_path() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(cache.join("gamebus-presenced").join("umu-protonfixes.txt"))
}

/// Fetch the list and cache it. One request; the response is parsed before
/// it replaces the cache, and the write is atomic like every other here.
pub(super) fn fetch(url: &str) -> Result<(ProtonFixes, usize), String> {
    let body = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| format!("{url}: {e}"))?
        .into_string()
        .map_err(|e| format!("reading the response body: {e}"))?;
    let fixes = ProtonFixes::parse_tree(&body)?;
    let n = fixes.len();
    if let Some(path) = cache_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, fixes.cache_text())
            .and_then(|()| std::fs::rename(&tmp, &path))
            .is_err()
        {
            // A cache that cannot be written costs a request next time,
            // nothing more - the list in hand is still good.
            let _ = std::fs::remove_file(&tmp);
        }
    }
    Ok((fixes, n))
}

/// A cache this old may not know a fix that landed since, and a missing fix
/// is what holds an entry out of the export - so it refreshes itself.
const STALE_AFTER: Duration = Duration::from_secs(7 * 86_400);

/// A local umu-protonfixes checkout (or an exported list), named by
/// `GAMEBUS_UMU_PROTONFIXES` - the fix-list twin of `GAMEBUS_UMU_DB`. Points
/// the scope check at a working copy, and keeps the tests off the network.
pub(super) const LOCAL_ENV: &str = "GAMEBUS_UMU_PROTONFIXES";

/// The fix list for a verification run: the local checkout when one is
/// named, else the cache when it is fresh, else one fetch. Returns the
/// reason instead when none works - the caller says so and leaves every
/// earlier verdict alone rather than claiming "no fix".
pub(super) fn load_for_verify() -> Result<ProtonFixes, String> {
    if let Some(local) = std::env::var_os(LOCAL_ENV) {
        return load_local(std::path::Path::new(&local));
    }
    let cached = cache_path().filter(|p| p.exists());
    let fresh = cached.as_ref().is_some_and(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < STALE_AFTER)
    });
    if fresh {
        if let Some(fixes) = cached
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| ProtonFixes::parse_cache(&raw).ok())
        {
            return Ok(fixes);
        }
    }
    match fetch(&endpoints().umu_protonfixes_tree) {
        Ok((fixes, _)) => Ok(fixes),
        Err(e) => {
            // Stale beats nothing: an old list still proves the fixes it
            // names, and the export gate only needs "does one exist".
            if let Some(fixes) = cached
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|raw| ProtonFixes::parse_cache(&raw).ok())
            {
                return Ok(fixes);
            }
            Err(e)
        }
    }
}

/// A umu-protonfixes checkout (its `gamefixes-*` directories are read
/// straight off the disk) or a file holding the paths, one per line. An
/// explicitly named source that yields nothing is an error, never a silent
/// "nothing needs a fix" - that verdict would hold every entry back.
fn load_local(path: &std::path::Path) -> Result<ProtonFixes, String> {
    if !path.is_dir() {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        return ProtonFixes::parse_cache(&raw).map_err(|e| format!("{}: {e}", path.display()));
    }
    let mut paths = Vec::new();
    let dirs = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
    for dir in dirs.flatten() {
        let name = dir.file_name().to_string_lossy().to_string();
        if !name.starts_with("gamefixes-") {
            continue;
        }
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            paths.push(format!("{name}/{}", file.file_name().to_string_lossy()));
        }
    }
    ProtonFixes::from_paths(paths.into_iter()).map_err(|e| {
        format!(
            "{}: {e} (is this a umu-protonfixes checkout?)",
            path.display()
        )
    })
}

/// protonfixes' own local fix directory. It expands `~` itself rather than
/// following XDG, so this does the same.
fn local_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/protonfixes/localfixes"))
}

/// The local fixes, indexed by umu id. Named by the raw `GAMEID` they serve:
/// `umu-<id>.py`, or the bare appid for Steam games. A missing directory is
/// simply no local fixes.
#[derive(Debug, Default)]
pub(super) struct LocalFixes {
    by_umu_id: HashMap<String, Vec<String>>,
}

impl LocalFixes {
    pub(super) fn load() -> Self {
        local_dir().map(|d| Self::read(&d)).unwrap_or_default()
    }

    fn read(dir: &std::path::Path) -> Self {
        let mut by_umu_id: HashMap<String, Vec<String>> = HashMap::new();
        for file in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = file.file_name().to_string_lossy().to_string();
            let Some(id) = local_umu_id_of(&name) else {
                continue;
            };
            by_umu_id
                .entry(id)
                .or_default()
                .push(file.path().to_string_lossy().to_string());
        }
        for paths in by_umu_id.values_mut() {
            paths.sort();
        }
        Self { by_umu_id }
    }

    pub(super) fn fixes_for(&self, umu_id: &str) -> &[String] {
        self.by_umu_id
            .get(&umu_id.to_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

fn local_umu_id_of(file: &str) -> Option<String> {
    let name = file.strip_suffix(".py")?;
    let id = if name.starts_with("umu-") && name.len() > 4 {
        name.to_string()
    } else if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
        format!("umu-{name}")
    } else {
        return None;
    };
    Some(id.to_lowercase())
}

/// A fix file's page upstream, for the merge request's evidence line.
pub(super) fn fix_url(path: &str) -> String {
    format!("{}/{path}", endpoints().umu_protonfixes_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real shape, trimmed: GitHub's recursive tree listing of
    /// umu-protonfixes (captured 2026-08-22).
    const TREE: &str = r#"{"sha":"abc","tree":[
        {"path":"README.md","type":"blob"},
        {"path":"gamefixes-steam","type":"tree"},
        {"path":"gamefixes-steam/__init__.py","type":"blob"},
        {"path":"gamefixes-steam/default.py","type":"blob"},
        {"path":"gamefixes-steam/1174180.py","type":"blob"},
        {"path":"gamefixes-egs/umu-1174180.py","type":"blob"},
        {"path":"gamefixes-gog/umu-1141086411.py","type":"blob"},
        {"path":"gamefixes-umu/umu-genshin.py","type":"blob"}
        ],"truncated":false}"#;

    #[test]
    fn the_tree_listing_indexes_fixes_by_the_id_they_serve() {
        let fixes = ProtonFixes::parse_tree(TREE).expect("real tree shape");
        // Steam files are named by the bare appid; the id is umu-<appid>.
        assert_eq!(
            fixes.fixes_for("umu-1174180"),
            ["gamefixes-egs/umu-1174180.py", "gamefixes-steam/1174180.py"]
        );
        assert_eq!(
            fixes.fixes_for("umu-1141086411"),
            ["gamefixes-gog/umu-1141086411.py"]
        );
        assert_eq!(
            fixes.fixes_for("umu-genshin"),
            ["gamefixes-umu/umu-genshin.py"]
        );
        // Control: no fix upstream, so no database row is warranted.
        assert!(fixes.fixes_for("umu-870780").is_empty());
        // Ids are matched case-insensitively, like every other lookup here.
        assert_eq!(fixes.fixes_for("UMU-Genshin").len(), 1);
        assert_eq!(fixes.len(), 3);
    }

    #[test]
    fn shared_machinery_is_never_read_as_a_fix() {
        assert_eq!(umu_id_of("gamefixes-steam/__init__.py"), None);
        assert_eq!(umu_id_of("gamefixes-gog/default.py"), None);
        assert_eq!(umu_id_of("README.md"), None);
        assert_eq!(umu_id_of("patches/something.patch"), None);
        // A bare name outside gamefixes-steam is not an appid, so it names
        // no id we could match a draft against.
        assert_eq!(umu_id_of("gamefixes-gog/2049187585.py"), None);
    }

    #[test]
    fn local_fixes_are_indexed_by_the_gameid_they_serve() {
        let dir = std::env::temp_dir().join(format!("gamebus-localfixes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "umu-1227690.py",
            "2840770.py",
            "default.py",
            "__init__.py",
            "notes.txt",
        ] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let local = LocalFixes::read(&dir);
        assert_eq!(local.fixes_for("umu-1227690").len(), 1);
        // A Steam GAMEID is the bare appid.
        assert_eq!(local.fixes_for("UMU-2840770").len(), 1);
        assert_eq!(local.by_umu_id.len(), 2);
        assert!(LocalFixes::read(&dir.join("absent"))
            .fixes_for("umu-1")
            .is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_cache_round_trips_the_listing() {
        let fixes = ProtonFixes::parse_tree(TREE).expect("real tree shape");
        let text = fixes.cache_text();
        let reloaded = ProtonFixes::parse_cache(&text).expect("own cache");
        assert_eq!(reloaded.len(), fixes.len());
        assert_eq!(
            reloaded.fixes_for("umu-1174180"),
            fixes.fixes_for("umu-1174180")
        );
        // An empty or fix-free listing is an error, not an "everything runs
        // fine" verdict that would hold every entry back.
        assert!(ProtonFixes::parse_cache("").is_err());
        assert!(ProtonFixes::parse_tree(r#"{"tree":[{"path":"README.md"}]}"#).is_err());
    }
}
