//! The stores gamebus knows, compiled from gamebus-gamedb's `stores.toml`
//! (see `build.rs`). The list is data there, not code here: a new store or a
//! launcher's new spelling for one is a change to the data set.

/// One store, in umu-database's spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Store {
    pub id: &'static str,
    /// Other spellings launchers use for it.
    pub aliases: &'static [&'static str],
    /// Whether a store picker offers it.
    pub pick: bool,
    /// umu-database's word for "no store"; never a gamedb page key.
    pub standalone: bool,
}

include!(concat!(env!("OUT_DIR"), "/stores.rs"));

/// Translate a launcher's store name into umu-database's spelling. Anything
/// that names no known store (`flathub`, empty, `unknown`) is the standalone
/// id, `none`: the label is a guess and says so.
pub fn normalize_store(raw: &str) -> &'static str {
    let raw = raw.trim().to_ascii_lowercase();
    STORES
        .iter()
        .find(|s| s.id == raw || s.aliases.contains(&raw.as_str()))
        .map_or(STANDALONE, |s| s.id)
}

/// The ids a store picker offers, in the data set's order.
pub fn pickable_stores() -> impl Iterator<Item = &'static str> {
    STORES.iter().filter(|s| s.pick).map(|s| s.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_spellings_translate_and_anything_else_is_standalone() {
        assert_eq!(normalize_store("ea_app"), "ea");
        assert_eq!(normalize_store(" SteamWindows "), "steam");
        assert_eq!(normalize_store("epic"), "egs");
        assert_eq!(normalize_store("gog"), "gog");
        assert_eq!(normalize_store("flathub"), "none");
        assert_eq!(normalize_store(""), "none");
    }

    #[test]
    fn the_picker_offers_real_stores_and_ends_with_the_way_out() {
        let picks: Vec<&str> = pickable_stores().collect();
        assert_eq!(picks.last(), Some(&STANDALONE));
        assert!(
            !picks.contains(&"umu"),
            "umu is a namespace, not a store to pick"
        );
        for id in &picks {
            assert_eq!(normalize_store(id), *id);
        }
    }
}
