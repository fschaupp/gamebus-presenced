//! Reading a gamedb data set off disk.
//!
//! Pages stay as parsed TOML rather than becoming typed structs. The lint has
//! to see what a contributor wrote, including the shapes a typed model would
//! reject before it could report them, and `preserve_order` keeps the
//! document's own key order so both linters walk a page the same way and
//! report its problems in the same sequence.

use std::fmt;
use std::path::{Path, PathBuf};

/// Stores whose STORE_PRECEDENCE position lends a page its canonical id,
/// best first. `steam` and `umu` are handled separately in
/// [`Page::candidates`], one rung above this list: `[ids]` outranks a store
/// entry naming the same authority, but a `[[stores.steam]]` or
/// `[[stores.umu]]` codename justifies the id just the same when `[ids]`
/// never recorded it.
pub const STORE_PRECEDENCE: [&str; 9] = [
    "gog",
    "egs",
    "ubisoft",
    "ea",
    "battlenet",
    "amazon",
    "humble",
    "itchio",
    "zoomplatform",
];

/// Anything that stopped the run before a report could be produced. A finding
/// about the data is not one of these - that is a line in the report.
#[derive(Debug)]
pub enum Error {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Toml {
        path: PathBuf,
        source: toml::de::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    Data(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Toml { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Json { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Data(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub fn read_toml(path: &Path) -> Result<toml::Value> {
    read_to_string(path)?.parse().map_err(|source| Error::Toml {
        path: path.to_path_buf(),
        source,
    })
}

pub fn read_json(path: &Path) -> Result<serde_json::Value> {
    serde_json::from_str(&read_to_string(path)?).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// One game page: its file name and the TOML it holds.
pub struct Page {
    /// File name with extension, as every message names it (`control.toml`).
    pub file: String,
    /// File name without extension - the human label, never the key.
    pub slug: String,
    pub path: PathBuf,
    pub doc: toml::Value,
}

impl Page {
    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.doc.get(key)?.as_str()
    }

    pub fn int_field(&self, key: &str) -> Option<i64> {
        self.doc.get(key)?.as_integer()
    }

    /// `[ids]`, the identifiers this page records in other people's
    /// namespaces.
    pub fn ids(&self) -> Option<&toml::Table> {
        self.doc.get("ids")?.as_table()
    }

    pub fn id_field(&self, key: &str) -> Option<&toml::Value> {
        self.ids()?.get(key)
    }

    /// Top-level `exe = [...]`, for a page with no store identity at all.
    pub fn exes(&self) -> Vec<&str> {
        self.doc
            .get("exe")
            .and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(toml::Value::as_str).collect())
            .unwrap_or_default()
    }

    pub fn merged_from(&self) -> Vec<&str> {
        self.doc
            .get("merged_from")
            .and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(toml::Value::as_str).collect())
            .unwrap_or_default()
    }

    /// `[[stores.<store>]]` in the document's own order, so a report walks a
    /// page the way it is written.
    pub fn stores(&self) -> Vec<(&str, Vec<&toml::Table>)> {
        let Some(stores) = self.doc.get("stores").and_then(toml::Value::as_table) else {
            return Vec::new();
        };
        stores
            .iter()
            .map(|(store, entries)| {
                let entries = entries
                    .as_array()
                    .map(|a| a.iter().filter_map(toml::Value::as_table).collect())
                    .unwrap_or_default();
                (store.as_str(), entries)
            })
            .collect()
    }

    pub fn has_stores(&self) -> bool {
        self.doc
            .get("stores")
            .and_then(toml::Value::as_table)
            .is_some_and(|t| !t.is_empty())
    }

    /// Every id this page's own data could justify, best first.
    ///
    /// `[ids].steam` outranks `[ids].umu`, which outranks a `stores.steam`
    /// or `stores.umu` entry naming the same authority (a steam store
    /// codename IS a steam appid), which outranks the stores in
    /// [`STORE_PRECEDENCE`] order.
    pub fn candidates(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(steam) = self.id_field("steam") {
            out.push(format!("steam-{}", scalar(steam)));
        }
        if let Some(umu) = self.id_field("umu").and_then(toml::Value::as_str) {
            out.push(umu.to_string());
        }
        let stores = self.stores();
        for (store, entries) in &stores {
            if *store != "steam" {
                continue;
            }
            for entry in entries {
                if let Some(codename) = entry.get("codename").and_then(toml::Value::as_str) {
                    out.push(format!("steam-{codename}"));
                }
            }
        }
        for (store, entries) in &stores {
            if *store != "umu" {
                continue;
            }
            for entry in entries {
                if let Some(codename) = entry.get("codename").and_then(toml::Value::as_str) {
                    out.push(codename.to_string());
                }
            }
        }
        for wanted in STORE_PRECEDENCE {
            for (store, entries) in &stores {
                if *store != wanted {
                    continue;
                }
                for entry in entries {
                    if let Some(codename) = entry.get("codename").and_then(toml::Value::as_str) {
                        out.push(format!("{store}-{codename}"));
                    }
                }
            }
        }
        out
    }

    /// The id precedence would assign to this page today. `None` when nothing
    /// names the game, which is the only case where an id is minted.
    pub fn derived_id(&self) -> Option<String> {
        self.candidates().into_iter().next()
    }

    /// The page's canonical id: what it wrote, or what precedence derives.
    pub fn canonical_id(&self) -> Option<String> {
        self.str_field("gamedb")
            .map(str::to_string)
            .or_else(|| self.derived_id())
    }
}

/// A TOML scalar as Python's `str()` would render it, for building
/// `steam-<appid>` out of an integer.
pub fn scalar(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        other => other.to_string(),
    }
}

/// The basename of an executable as recorded, lowercased.
///
/// A store entry may carry a path (`Content/Bin/Game.exe`); the identity is
/// the last component. Both separators count, because these paths are
/// observed on Windows-facing launchers.
pub fn exe_basename(raw: &str) -> String {
    raw.rsplit(['/', '\\']).next().unwrap_or(raw).to_lowercase()
}

/// A data set: the game pages, `helpers.toml`, and where the schemas live.
pub struct DataSet {
    pub pages: Vec<Page>,
    pub helpers: toml::Value,
    pub helpers_path: PathBuf,
    pub schema_dir: PathBuf,
}

impl DataSet {
    /// Load `<base>/games/*.toml` and `<base>/helpers.toml`.
    ///
    /// Schemas come from `<base>/schema` when that exists and from
    /// `gamedb/schema` relative to the working directory otherwise, which is
    /// what `.scripts/gamedb-lint.py` does and is how the fixture set - which
    /// carries no schemas of its own - gets validated.
    pub fn load(base: &Path, schema_dir: Option<&Path>) -> Result<Self> {
        let schema_dir = match schema_dir {
            Some(dir) => dir.to_path_buf(),
            None if base.join("schema").is_dir() => base.join("schema"),
            None => PathBuf::from("gamedb/schema"),
        };

        let games = base.join("games");
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&games)
            .map_err(|source| Error::Io {
                path: games.clone(),
                source,
            })?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            // glob("*.toml") skips dotfiles; so does this.
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !n.starts_with('.'))
            })
            .collect();
        // `sorted(Path.glob(...))` compares whole paths; one directory makes
        // that the same as comparing file names.
        paths.sort();

        let mut pages = Vec::with_capacity(paths.len());
        for path in paths {
            let file = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let slug = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let doc = read_toml(&path)?;
            pages.push(Page {
                file,
                slug,
                path,
                doc,
            });
        }

        let helpers_path = base.join("helpers.toml");
        let helpers = read_toml(&helpers_path)?;

        Ok(Self {
            pages,
            helpers,
            helpers_path,
            schema_dir,
        })
    }

    /// `[[helper]]` entries in the order the file writes them.
    pub fn helper_entries(&self) -> Vec<&toml::Table> {
        self.helpers
            .get("helper")
            .and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(toml::Value::as_table).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(doc: &str) -> Page {
        Page {
            file: "x.toml".into(),
            slug: "x".into(),
            path: PathBuf::from("x.toml"),
            doc: doc.parse().expect("fixture parses"),
        }
    }

    #[test]
    fn a_steam_app_id_outranks_everything_else() {
        let p = page(
            "title = \"Control\"\n[ids]\nsteam = 870780\nsource = \"steam-sku\"\nseen = \"2026-08-22\"\n\
             \n[[stores.egs]]\ncodename = \"Calluna\"\nseen = \"2026-08-15\"\nsource = \"manual\"\nconfidence = \"high\"\n",
        );
        assert_eq!(p.derived_id().as_deref(), Some("steam-870780"));
        assert_eq!(p.candidates(), ["steam-870780", "egs-Calluna"]);
    }

    #[test]
    fn a_numeric_gog_product_id_is_a_perfectly_good_id() {
        let p = page(
            "title = \"Prey\"\n[[stores.gog]]\ncodename = \"1207658770\"\nseen = \"2026-08-22\"\n\
             source = \"manual\"\nconfidence = \"high\"\n",
        );
        assert_eq!(p.derived_id().as_deref(), Some("gog-1207658770"));
    }

    #[test]
    fn store_precedence_beats_the_documents_own_order() {
        let p = page(
            "title = \"X\"\n[[stores.humble]]\ncodename = \"h\"\nseen = \"2026-08-22\"\nsource = \"manual\"\nconfidence = \"high\"\n\
             \n[[stores.gog]]\ncodename = \"g\"\nseen = \"2026-08-22\"\nsource = \"manual\"\nconfidence = \"high\"\n",
        );
        assert_eq!(p.derived_id().as_deref(), Some("gog-g"));
    }

    #[test]
    fn a_page_that_names_nothing_derives_nothing() {
        assert_eq!(page("title = \"X\"\n").derived_id(), None);
    }

    #[test]
    fn an_executable_is_identified_by_its_basename() {
        assert_eq!(exe_basename("Content/Bin/Game.EXE"), "game.exe");
        assert_eq!(exe_basename("C:\\Games\\Game.exe"), "game.exe");
        assert_eq!(exe_basename("Game.exe"), "game.exe");
    }
}
