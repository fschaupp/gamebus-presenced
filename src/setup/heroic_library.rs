//! Heroic's store_cache libraries — the second zero-network candidate
//! source for the misses pane's `p`: the user's own libraries map titles to
//! the store identities Heroic launches with.
//!
//! One JSON cache per store under `<config>/heroic/store_cache/`, tried in
//! the flatpak's config dir first (a plain ~/.config copy may exist beside
//! it, empty): `legendary_library.json` carries a `library` array whose
//! `app_name` is the EGS Builds App Name the umu database wants verbatim;
//! `gog_library.json` carries a `games` array whose `app_name` is the
//! numeric GOG product id. Both shapes verified against a live Heroic
//! (flatpak, 2026-08-08).
//!
//! Everything parses defensively: a missing, empty, or malformed cache is an
//! empty library, never an error — `p` must behave identically on a machine
//! without Heroic.

use std::path::PathBuf;

use serde::Deserialize;

/// One game of a Heroic library: a title and the identity the store
/// launches it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryGame {
    pub title: String,
    /// umu-database store id (`egs` / `gog`).
    pub store: String,
    /// What `HEROIC_APP_NAME` carries at launch: the EGS Builds App Name or
    /// the numeric GOG product id — NOT the (lowercase) EGS namespace.
    pub codename: String,
}

/// Library candidates for a title, both stores, ranked and capped per store
/// by [`search`]. Local files only — never the network.
pub(crate) fn candidates(query: &str) -> Vec<LibraryGame> {
    let mut out = search(&load("legendary_library.json", parse_legendary), query);
    out.extend(search(&load("gog_library.json", parse_gog), query));
    out
}

/// The first cache file that reads wins for its store; whatever it fails to
/// yield is an empty library, not a fallthrough — a present-but-broken
/// flatpak cache must not be shadowed by a stale plain-config one.
fn load(file: &str, parse: fn(&str) -> Vec<LibraryGame>) -> Vec<LibraryGame> {
    cache_paths(file)
        .into_iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|raw| parse(&raw))
        .unwrap_or_default()
}

/// Flatpak config dir first, then the ordinary one.
fn cache_paths(file: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.push(
            home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/store_cache")
                .join(file),
        );
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    {
        paths.push(config.join("heroic/store_cache").join(file));
    }
    paths
}

/// The fields shared by both caches' game objects; everything else in them
/// (art, install records, namespaces) is ignored.
#[derive(Deserialize)]
struct RawGame {
    app_name: Option<String>,
    title: Option<String>,
}

fn games(raw: Vec<RawGame>, store: &str) -> Vec<LibraryGame> {
    raw.into_iter()
        .filter_map(|g| {
            Some(LibraryGame {
                title: g.title.filter(|t| !t.is_empty())?,
                store: store.to_string(),
                codename: g.app_name.filter(|a| !a.is_empty())?,
            })
        })
        .collect()
}

fn parse_legendary(raw: &str) -> Vec<LibraryGame> {
    #[derive(Deserialize)]
    struct Cache {
        #[serde(default)]
        library: Vec<RawGame>,
    }
    serde_json::from_str::<Cache>(raw)
        .map(|c| games(c.library, "egs"))
        .unwrap_or_default()
}

fn parse_gog(raw: &str) -> Vec<LibraryGame> {
    #[derive(Deserialize)]
    struct Cache {
        #[serde(default)]
        games: Vec<RawGame>,
    }
    serde_json::from_str::<Cache>(raw)
        .map(|c| games(c.games, "gog"))
        .unwrap_or_default()
}

/// Same matching spirit as `UmuDb::search_title`: case-insensitive substring
/// in both directions, ranked exact match, then title-starts-with-query,
/// then the rest; deduped by (store, codename) and capped at 10 — this runs
/// per store, so the cap is per store too.
pub(crate) fn search(games: &[LibraryGame], query: &str) -> Vec<LibraryGame> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut seen: std::collections::HashSet<(String, String)> = Default::default();
    let mut ranked: Vec<(u8, &LibraryGame)> = Vec::new();
    for g in games {
        let t = g.title.to_lowercase();
        let rank = if t == q {
            0
        } else if t.starts_with(&q) {
            1
        } else if t.contains(&q) || q.contains(&t) {
            2
        } else {
            continue;
        };
        if seen.insert((g.store.to_lowercase(), g.codename.to_lowercase())) {
            ranked.push((rank, g));
        }
    }
    // Stable sort: within a rank, library order stands.
    ranked.sort_by_key(|&(rank, _)| rank);
    ranked
        .into_iter()
        .take(10)
        .map(|(_, g)| g.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured shape of the flatpak's legendary cache (2026-08-08): the
    /// `library` array under a top-level object, extra fields and all — note
    /// the lowercase `namespace` beside the `app_name` the database wants.
    const LEGENDARY: &str = r#"{
        "library": [
            {"app_name":"Calluna","title":"Control","namespace":"calluna",
             "runner":"legendary","is_installed":true,"store_url":null,
             "install":{"executable":"Control.exe","is_dlc":false}},
            {"app_name":"Catnip","title":"Borderlands 3","namespace":"catnip",
             "runner":"legendary","is_installed":false},
            {"app_name":"Ginger","title":"Control Ultimate Edition","namespace":"ginger",
             "runner":"legendary","is_installed":false}
        ],
        "__timestamp": {"library": "Sat Aug 08 2026 13:47:14 GMT+0200 (Mitteleuropäische Sommerzeit)"}
    }"#;

    /// Captured shape of the GOG cache: a `games` array, numeric product ids
    /// as `app_name` — plus Heroic's own non-game redist entry.
    const GOG: &str = r#"{
        "games": [
            {"app_name":"gog-redist","title":"Galaxy Common Redistributables","runner":"gog","is_installed":true},
            {"app_name":"1158493447","title":"Prey","runner":"gog","is_installed":true},
            {"app_name":"2049187585","title":"Control Ultimate Edition","runner":"gog","is_installed":false}
        ],
        "__timestamp": {"games": "Sat Aug 08 2026 14:37:16 GMT+0200 (Mitteleuropäische Sommerzeit)"}
    }"#;

    #[test]
    fn both_real_cache_shapes_parse_to_store_identities() {
        let egs = parse_legendary(LEGENDARY);
        assert_eq!(egs.len(), 3);
        assert_eq!(egs[0].title, "Control");
        assert_eq!(egs[0].store, "egs");
        // The Builds App Name, capitalized — never the lowercase namespace.
        assert_eq!(egs[0].codename, "Calluna");

        let gog = parse_gog(GOG);
        assert_eq!(gog.len(), 3);
        assert_eq!(gog[1].title, "Prey");
        assert_eq!(gog[1].store, "gog");
        assert_eq!(gog[1].codename, "1158493447");
    }

    #[test]
    fn missing_or_malformed_caches_are_empty_never_an_error() {
        assert!(parse_legendary("").is_empty());
        assert!(parse_legendary("{}").is_empty()); // Heroic's untouched cache
        assert!(parse_legendary("not json at all").is_empty());
        assert!(parse_legendary("[1,2,3]").is_empty());
        assert!(parse_gog(r#"{"games":"not an array"}"#).is_empty());
        // Entries missing a field drop out; the rest survive.
        let partial = parse_gog(
            r#"{"games":[{"title":"No App Name"},{"app_name":"1"},
            {"app_name":"1207658883","title":"Age of Wonders"}]}"#,
        );
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].codename, "1207658883");
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_the_rest() {
        let games = parse_legendary(LEGENDARY);
        let hits = search(&games, "control");
        let titles: Vec<&str> = hits.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, vec!["Control", "Control Ultimate Edition"]);
        // Case-insensitive, and both substring directions: a decorated
        // launcher title still finds the plain library row.
        assert_eq!(search(&games, "CONTROL")[0].codename, "Calluna");
        assert_eq!(search(&games, "Control GOTY Deluxe").len(), 1);
        assert!(search(&games, "Half-Life").is_empty());
        assert!(search(&games, "  ").is_empty());
    }

    #[test]
    fn search_dedups_and_caps_at_ten() {
        let mut games = vec![
            LibraryGame {
                title: "Fixture Quest".into(),
                store: "egs".into(),
                codename: "Dup".into(),
            };
            3
        ];
        for i in 0..15 {
            games.push(LibraryGame {
                title: format!("Fixture Quest {i:02}"),
                store: "egs".into(),
                codename: format!("code{i}"),
            });
        }
        let hits = search(&games, "fixture quest");
        assert_eq!(hits.len(), 10);
        assert_eq!(hits[0].codename, "Dup"); // exact match ranks first, once
    }
}
