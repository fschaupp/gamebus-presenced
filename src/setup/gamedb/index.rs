//! What gamebus-gamedb already knows: the published identity index.
//!
//! The data set builds `identities.json` on every release - four flat
//! tables, of which three matter here. `aliases` (every identifier that
//! resolves to a page) answers "does a page for this game exist already?",
//! which is the first thing `gamedb/CONTRIBUTING.md` asks a contributor to
//! check. `games` and `stores` answer the follow-up: does that page already
//! carry everything this machine knows, or is there something to add?
//!
//! Cached exactly like the protonfix list next door (`umu_misses::fixes`):
//! one request on the user's word, a sidecar recording when it happened and
//! which release it came from, a staleness window, and an environment
//! override so tests never touch the network.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::{endpoints, HTTP_TIMEOUT, USER_AGENT};

/// The published index, reduced to the lookups the export needs.
#[derive(Debug, Default)]
pub(super) struct GamedbIndex {
    /// Every alias the data set publishes → the page's canonical id.
    by_alias: HashMap<String, String>,
    /// Canonical id → the page's title, for "already in gamebus-gamedb as".
    titles: HashMap<String, String>,
    /// Canonical id → the page's file stem (`control`). Absent in an index
    /// built before the column existed; see [`GamedbIndex::page`].
    pages: HashMap<String, String>,
    /// Canonical id → `games.steam`. A missing entry is what makes a Steam
    /// app id worth contributing.
    steam: HashMap<String, u64>,
    /// `(canonical id, store, codename)` for every row of `stores`: whether
    /// a store identity this machine saw is already on the page.
    store_rows: HashSet<(String, String, String)>,
}

impl GamedbIndex {
    /// Parse `identities.json`. Unknown tables and columns are ignored, so a
    /// newer artifact than this binary knows about still loads.
    pub(super) fn parse(raw: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            games: Vec<RawGame>,
            #[serde(default)]
            stores: Vec<RawStore>,
            #[serde(default)]
            aliases: Vec<RawAlias>,
        }
        #[derive(Deserialize)]
        struct RawGame {
            id: String,
            #[serde(default)]
            title: String,
            /// Added in gamedb-build 0.1.1; the published index predates it.
            #[serde(default)]
            page: Option<String>,
            #[serde(default)]
            steam: Option<u64>,
        }
        #[derive(Deserialize)]
        struct RawStore {
            id: String,
            store: String,
            codename: String,
        }
        #[derive(Deserialize)]
        struct RawAlias {
            alias: String,
            id: String,
        }
        let raw: Raw =
            serde_json::from_str(raw).map_err(|e| format!("not a gamebus-gamedb index: {e}"))?;
        let mut titles = HashMap::with_capacity(raw.games.len());
        let mut pages = HashMap::new();
        let mut steam = HashMap::new();
        for game in raw.games {
            if let Some(page) = game.page.filter(|p| !p.is_empty()) {
                pages.insert(game.id.clone(), page);
            }
            if let Some(appid) = game.steam {
                steam.insert(game.id.clone(), appid);
            }
            titles.insert(game.id, game.title);
        }
        let store_rows: HashSet<(String, String, String)> = raw
            .stores
            .into_iter()
            .map(|s| (s.id, s.store, s.codename))
            .collect();
        let by_alias: HashMap<String, String> =
            raw.aliases.into_iter().map(|a| (a.alias, a.id)).collect();
        Ok(Self {
            by_alias,
            titles,
            pages,
            steam,
            store_rows,
        })
    }

    /// The page one identifier resolves to: its canonical id and title.
    /// Aliases are matched verbatim - a store codename is case-sensitive by
    /// the data set's own rule, and the `exe:` aliases are already
    /// lowercased by the build.
    pub(super) fn resolve(&self, alias: &str) -> Option<(&str, &str)> {
        let id = self.by_alias.get(alias)?;
        let title = self.titles.get(id).map(String::as_str).unwrap_or("");
        Some((id.as_str(), title))
    }

    /// The file `games/<page>.toml` this game's page lives in. `None` for an
    /// index built before the column existed; the caller falls back to the
    /// title's slug, which the file name derives from anyway.
    pub(super) fn page(&self, id: &str) -> Option<&str> {
        self.pages.get(id).map(String::as_str)
    }

    /// The Steam app id the page records, if any.
    pub(super) fn steam(&self, id: &str) -> Option<u64> {
        self.steam.get(id).copied()
    }

    /// Whether the page already lists this store product.
    pub(super) fn has_store(&self, id: &str, store: &str, codename: &str) -> bool {
        self.store_rows
            .contains(&(id.to_string(), store.to_string(), codename.to_string()))
    }

    /// How many games the data set carries.
    pub(super) fn len(&self) -> usize {
        self.titles.len()
    }
}

/// Where the fetched index is cached, beside the umu caches.
pub(super) fn cache_path() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(
        cache
            .join("gamebus-presenced")
            .join("gamedb-identities.json"),
    )
}

/// The sidecar beside the cache: when it was fetched, and from which
/// release. `key = value` lines, parsed by hand like `endpoints.toml`.
fn meta_path() -> Option<PathBuf> {
    Some(cache_path()?.with_extension("meta"))
}

/// A cache older than this may not know a page that landed since, and a
/// missing page is what lets an export write a duplicate. Same window as
/// the protonfix list.
pub(super) const STALE_AFTER: Duration = Duration::from_secs(7 * 86_400);

/// A local `identities.json` (a build of your own checkout, say), named by
/// `GAMEBUS_GAMEDB_INDEX` - the gamedb twin of `GAMEBUS_UMU_PROTONFIXES`.
/// Always treated as fresh: a file you pointed at is yours to keep current.
pub(super) const LOCAL_ENV: &str = "GAMEBUS_GAMEDB_INDEX";

/// The index in hand, with everything needed to say how much to trust it.
pub(super) struct Loaded {
    pub(super) index: GamedbIndex,
    /// The date the cache records for its own fetch, `YYYY-MM-DD`.
    pub(super) fetched: Option<String>,
    /// The release tag the cached copy came from.
    pub(super) release: Option<String>,
    /// How long ago the cache was written.
    pub(super) age: Option<Duration>,
    pub(super) stale: bool,
    /// True when it came from [`LOCAL_ENV`] rather than the cache.
    pub(super) local: bool,
}

/// A stand-in for an index that is not there, for an export the user
/// explicitly confirmed without one. It resolves nothing, which is exactly
/// what "nothing was checked" means - the caller says so in words.
pub(super) fn empty() -> Loaded {
    Loaded {
        index: GamedbIndex::default(),
        fetched: None,
        release: None,
        age: None,
        stale: false,
        local: false,
    }
}

/// What the fetch could not do. A 404 is the ordinary "nothing published
/// yet" case and reads as such; everything else keeps its own words.
pub(super) enum FetchError {
    /// The asset is not there: no data release exists yet.
    NoRelease,
    Other(String),
}

impl FetchError {
    pub(super) fn message(&self) -> String {
        match self {
            FetchError::NoRelease => "No gamebus-gamedb release is published yet.".to_string(),
            FetchError::Other(e) => e.clone(),
        }
    }
}

/// Fetch the index and cache it. The body is parsed before it replaces the
/// cache, and the write is atomic like every other here.
///
/// GitHub answers `releases/latest/download/<asset>` with a redirect to
/// `releases/download/<tag>/<asset>`, and THAT with a second redirect to the
/// asset store, whose URL carries no tag. So the first hop is resolved by
/// hand, without following it, to read the tag out of `Location`; the body
/// is then fetched from there with redirects on. Two requests, the first a
/// bodiless 302. A server that answers the first request with the body
/// outright (no redirect) still works; it just has no tag to give.
pub(super) fn fetch(url: &str) -> Result<(GamedbIndex, Option<String>), FetchError> {
    let first_hop = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(HTTP_TIMEOUT)
        .build();
    let response = first_hop
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| match &e {
            ureq::Error::Status(404, _) => FetchError::NoRelease,
            _ => FetchError::Other(format!("{url}: {e}")),
        })?;
    let (release, response) = match response.header("Location") {
        Some(location) if (300..400).contains(&response.status()) => {
            let release = release_tag(location);
            let followed = ureq::get(location)
                .set("User-Agent", USER_AGENT)
                .timeout(HTTP_TIMEOUT)
                .call()
                .map_err(|e| match &e {
                    ureq::Error::Status(404, _) => FetchError::NoRelease,
                    _ => FetchError::Other(format!("{location}: {e}")),
                })?;
            (release, followed)
        }
        _ => (release_tag(response.get_url()), response),
    };
    let body = response
        .into_string()
        .map_err(|e| FetchError::Other(format!("reading the response body: {e}")))?;
    let index = GamedbIndex::parse(&body).map_err(FetchError::Other)?;
    write_cache(&body, release.as_deref());
    Ok((index, release))
}

/// The release tag in a `releases/download/<tag>/<asset>` URL.
fn release_tag(url: &str) -> Option<String> {
    let rest = url.split_once("/releases/download/")?.1;
    let tag = rest.split('/').next()?;
    (!tag.is_empty()).then(|| tag.to_string())
}

/// Replace the cache and its sidecar. A cache that cannot be written costs
/// a request next time, nothing more - so every failure here is silent.
fn write_cache(body: &str, release: Option<&str>) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if std::fs::write(&tmp, body)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let Some(meta) = meta_path() else { return };
    let mut text = format!("fetched = \"{}\"\n", crate::umu_report::today());
    if let Some(release) = release {
        text.push_str(&format!("release = \"{release}\"\n"));
    }
    let _ = std::fs::write(meta, text);
}

/// The index for a listing or an export: the local file when one is named,
/// else the cache. `Ok(None)` means nothing is cached yet - the caller says
/// so and points at `--fetch`; this function never reaches the network.
pub(super) fn load() -> Result<Option<Loaded>, String> {
    if let Some(local) = std::env::var_os(LOCAL_ENV) {
        let path = PathBuf::from(local);
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let index = GamedbIndex::parse(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(Some(Loaded {
            index,
            fetched: None,
            release: None,
            age: None,
            stale: false,
            local: true,
        }));
    }
    let Some(path) = cache_path().filter(|p| p.exists()) else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read the cached index {}: {e}", path.display()))?;
    let index = GamedbIndex::parse(&raw).map_err(|e| {
        format!(
            "the cached index {} is unreadable ({e}) - re-run with --fetch",
            path.display()
        )
    })?;
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok());
    let (fetched, release) = read_meta();
    Ok(Some(Loaded {
        index,
        fetched,
        release,
        stale: age.is_some_and(|a| a >= STALE_AFTER),
        age,
        local: false,
    }))
}

/// `fetched` and `release` from the sidecar, if it is there and readable.
fn read_meta() -> (Option<String>, Option<String>) {
    let Some(path) = meta_path() else {
        return (None, None);
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    (value_of(&raw, "fetched"), value_of(&raw, "release"))
}

/// One `key = "value"` line out of the sidecar. Quotes optional, so a file
/// edited by hand still reads.
fn value_of(raw: &str, key: &str) -> Option<String> {
    for line in raw.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim().trim_matches('"').trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

/// How much to trust this copy, in one phrase: `release v2026.08.23,
/// fetched 2 days ago`. Shared by the CLI listing and the TUI pane.
pub(super) fn describe(loaded: &Loaded) -> String {
    if loaded.local {
        return "local file".to_string();
    }
    let mut parts = Vec::new();
    if let Some(release) = &loaded.release {
        parts.push(format!("release {release}"));
    }
    parts.push(match (loaded.age, &loaded.fetched) {
        (Some(age), _) => format!("fetched {}", ago(age)),
        (None, Some(date)) => format!("fetched {date}"),
        (None, None) => "fetched at an unknown time".to_string(),
    });
    parts.join(", ")
}

/// `today` / `1 day ago` / `N days ago`.
pub(super) fn ago(age: Duration) -> String {
    match age.as_secs() / 86_400 {
        0 => "today".to_string(),
        1 => "1 day ago".to_string(),
        n => format!("{n} days ago"),
    }
}

/// A real build of the data set as it stood on 2026-08-23, from
/// `gamedb-build --data gamedb`. Trimmed to the three tables this module
/// reads, which is exactly what a newer artifact carrying more of them has
/// to survive.
#[cfg(test)]
pub(super) const FIXTURE: &str = r#"{
  "schema_version": 1,
  "games": [
    {"id": "steam-868360", "title": "Project Hospital", "page": "project-hospital",
     "year": null, "variant_of": null, "note": "No protonfix exists upstream.",
     "steam": 868360, "umu": null},
    {"id": "steam-870780", "title": "Control", "page": "control", "year": null,
     "variant_of": null, "note": null, "steam": 870780, "umu": null}
  ],
  "stores": [
    {"id": "steam-868360", "store": "gog", "codename": "1660194629",
     "edition": null, "exe": "ProjectHospital.exe", "seen": "2026-08-16",
     "source": "manual", "confidence": "high"},
    {"id": "steam-870780", "store": "egs", "codename": "Calluna",
     "edition": null, "exe": null, "seen": "2026-08-15",
     "source": "heroic-config", "confidence": "high"}
  ],
  "aliases": [
    {"alias": "egs-Calluna", "id": "steam-870780"},
    {"alias": "exe:projecthospital.exe", "id": "steam-868360"},
    {"alias": "gog-1660194629", "id": "steam-868360"},
    {"alias": "steam-868360", "id": "steam-868360"},
    {"alias": "steam-870780", "id": "steam-870780"}
  ],
  "helpers": [
    {"exe": "unitycrashhandler64.exe", "reason": "Ships beside every Unity game.",
     "incident": null, "seen": "2026-08-08"}
  ]
}"#;

/// The index URL, with the endpoints file's override applied.
pub(super) fn identities_url() -> &'static str {
    &endpoints().gamedb_identities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_published_identifier_resolves_to_its_page() {
        let index = GamedbIndex::parse(super::FIXTURE).expect("a real build of the data set");
        assert_eq!(index.len(), 2);
        assert_eq!(
            index.resolve("steam-870780"),
            Some(("steam-870780", "Control"))
        );
        // A store codename and an executable resolve to the same page as the
        // canonical id - that is the whole point of the aliases table.
        assert_eq!(
            index.resolve("egs-Calluna"),
            Some(("steam-870780", "Control"))
        );
        assert_eq!(
            index.resolve("exe:projecthospital.exe"),
            Some(("steam-868360", "Project Hospital"))
        );
        assert_eq!(
            index.resolve("gog-1660194629"),
            Some(("steam-868360", "Project Hospital"))
        );
        // Codenames are case-sensitive upstream, so the lookup is too.
        assert_eq!(index.resolve("egs-calluna"), None);
        assert_eq!(index.resolve("steam-1985810"), None);
    }

    #[test]
    fn anything_that_is_not_an_index_is_refused() {
        assert!(GamedbIndex::parse("").is_err());
        assert!(GamedbIndex::parse("<!doctype html>").is_err());
        // An empty but well-formed artifact is not an error: a data set with
        // no pages yet resolves nothing, which is the truth.
        let empty = GamedbIndex::parse(r#"{"games":[],"aliases":[]}"#).expect("well-formed");
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.resolve("steam-1"), None);
    }

    #[test]
    fn the_release_tag_comes_out_of_the_url_github_redirected_to() {
        assert_eq!(
            release_tag(
                "https://github.com/fschaupp/gamebus-gamedb/releases/download/v2026.08.23/identities.json"
            )
            .as_deref(),
            Some("v2026.08.23")
        );
        // No redirect happened, or the layout changed: no tag, not a wrong one.
        assert_eq!(
            release_tag(
                "https://github.com/fschaupp/gamebus-gamedb/releases/latest/download/identities.json"
            ),
            None
        );
        assert_eq!(release_tag("https://example.invalid/x.json"), None);
    }

    #[test]
    fn the_sidecar_says_when_and_from_where() {
        let raw = "fetched = \"2026-08-21\"\nrelease = \"v2026.08.20\"\n";
        assert_eq!(value_of(raw, "fetched").as_deref(), Some("2026-08-21"));
        assert_eq!(value_of(raw, "release").as_deref(), Some("v2026.08.20"));
        assert_eq!(value_of(raw, "nothing"), None);
        // Hand-edited, unquoted, and with the release line missing.
        assert_eq!(
            value_of("fetched = 2026-08-21\n", "fetched").as_deref(),
            Some("2026-08-21")
        );
        assert_eq!(value_of("fetched = \"\"\n", "fetched"), None);
    }

    #[test]
    fn a_week_old_cache_is_stale_and_the_description_says_how_old() {
        let loaded = |age: Option<Duration>, local: bool| Loaded {
            index: GamedbIndex::default(),
            fetched: Some("2026-08-21".into()),
            release: Some("v2026.08.20".into()),
            stale: age.is_some_and(|a| a >= STALE_AFTER),
            age,
            local,
        };
        assert!(!loaded(Some(Duration::from_secs(6 * 86_400)), false).stale);
        assert!(loaded(Some(Duration::from_secs(7 * 86_400)), false).stale);
        assert_eq!(
            describe(&loaded(Some(Duration::from_secs(2 * 86_400)), false)),
            "release v2026.08.20, fetched 2 days ago"
        );
        assert_eq!(
            describe(&loaded(Some(Duration::from_secs(3600)), false)),
            "release v2026.08.20, fetched today"
        );
        assert_eq!(
            describe(&loaded(Some(Duration::from_secs(86_400)), false)),
            "release v2026.08.20, fetched 1 day ago"
        );
        // A file the user pointed at has no fetch date to report.
        assert_eq!(describe(&loaded(None, true)), "local file");
    }
}
