//! Turning the TOML pages into the published artifacts.
//!
//! The repo optimises for review, the artifacts optimise for lookup, and this
//! is the seam between them. One relational shape backs all of it:
//!
//! ```text
//! games(id PK, title, year, variant_of, note, steam, umu)
//! stores(id, store, codename, edition, exe, seen, source, confidence)
//! aliases(alias PK, id)
//! helpers(exe PK, reason, incident, seen)
//! ```
//!
//! `aliases` is the table that makes the set usable: every identifier that
//! resolves to a page has a row - the canonical id, each `<store>-<codename>`,
//! the Steam app id, the umu id, `exe:<basename>` for every executable named,
//! and everything absorbed through `merged_from`. A client resolves any slug
//! it has to exactly one page with one lookup.
//!
//! **Determinism is a requirement.** Rows are sorted by canonical id, then
//! store, then codename; column order is fixed; nothing carries a build
//! timestamp, and the source commit lives in `manifest.toml` alone so the
//! same data always produces the same bytes. `tests/build.rs` builds twice
//! and compares.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::model::{exe_basename, scalar, DataSet, Error, Result};
use crate::{emit, parquet_out, sqlite_out};

/// What the builder was asked to do.
pub struct Options {
    /// The data set: `games/`, `helpers.toml`, `schema/`.
    pub data: PathBuf,
    /// Where the artifacts go. Created if missing.
    pub out: PathBuf,
    /// The commit the data came from, for `manifest.toml`. `"unknown"` when
    /// nothing said.
    pub commit: String,
    /// Skip the lint. Only for building a set you already know is broken.
    pub skip_lint: bool,
    /// Where the schemas live. `None` takes `<data>/schema` when it exists
    /// and `gamedb/schema` relative to the working directory otherwise,
    /// which is what the lint does.
    pub schema_dir: Option<PathBuf>,
}

/// One row of `games`.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub year: Option<i64>,
    pub variant_of: Option<String>,
    pub note: Option<String>,
    /// `[ids].steam`, the Steam app id, where the page records one.
    pub steam: Option<i64>,
    /// `[ids].umu`, present only where umu-database really named the game.
    pub umu: Option<String>,
}

/// One row of `stores`: one product on one store.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct StoreRow {
    pub id: String,
    pub store: String,
    pub codename: String,
    pub edition: Option<String>,
    pub exe: Option<String>,
    pub seen: String,
    pub source: String,
    pub confidence: String,
}

/// One row of `aliases`: an identifier and the page it resolves to.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Alias {
    pub alias: String,
    pub id: String,
}

/// One row of `helpers`: an executable that can never identify a game.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Helper {
    pub exe: String,
    pub reason: String,
    pub incident: Option<String>,
    pub seen: String,
}

/// The whole data set in its relational shape, sorted and ready to write.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Tables {
    pub schema_version: i64,
    pub games: Vec<Game>,
    pub stores: Vec<StoreRow>,
    pub aliases: Vec<Alias>,
    pub helpers: Vec<Helper>,
}

/// One artifact, as `manifest.toml` records it.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// What a build produced.
#[derive(Debug)]
pub struct Outcome {
    pub tables: Tables,
    pub artifacts: Vec<Artifact>,
    pub warnings: Vec<String>,
}

/// The artifacts a build writes, in the order `manifest.toml` lists them.
///
/// `manifest.toml` is not among them: it is the thing carrying the checksums.
pub const ARTIFACTS: [&str; 4] = [
    "identities.json",
    "identities.parquet",
    "identities.sqlite",
    "shared-helpers.txt",
];

pub const MANIFEST: &str = "manifest.toml";

/// Load, validate, and write every artifact.
pub fn run(options: &Options) -> Result<Outcome> {
    let data = DataSet::load(&options.data, options.schema_dir.as_deref())?;

    let mut warnings = Vec::new();
    if !options.skip_lint {
        let report = crate::lint::check(&data);
        if !report.ok() {
            return Err(Error::Data(format!(
                "the data set does not validate, so there is nothing to build:\n{}",
                report.render().trim_end()
            )));
        }
        warnings = report.warnings;
    }

    let tables = tables(&data)?;

    std::fs::create_dir_all(&options.out).map_err(|source| Error::Io {
        path: options.out.clone(),
        source,
    })?;

    emit::json(&options.out.join("identities.json"), &tables)?;
    parquet_out::write(&options.out.join("identities.parquet"), &tables)?;
    sqlite_out::write(&options.out.join("identities.sqlite"), &tables)?;
    emit::shared_helpers(&options.out.join("shared-helpers.txt"), &tables)?;

    let mut artifacts = Vec::with_capacity(ARTIFACTS.len());
    for name in ARTIFACTS {
        artifacts.push(emit::checksum(&options.out, name)?);
    }
    emit::manifest(&options.out.join(MANIFEST), &options.commit, &artifacts)?;

    Ok(Outcome {
        tables,
        artifacts,
        warnings,
    })
}

/// Fold the pages and `helpers.toml` into the relational shape.
pub fn tables(data: &DataSet) -> Result<Tables> {
    let mut games: Vec<Game> = Vec::with_capacity(data.pages.len());
    let mut stores: Vec<StoreRow> = Vec::new();
    let mut aliases: Vec<Alias> = Vec::new();
    let mut alias_owner: HashMap<String, String> = HashMap::new();
    let mut game_owner: HashMap<String, String> = HashMap::new();

    for page in &data.pages {
        let file = page.file.as_str();
        let Some(id) = page.canonical_id() else {
            return Err(Error::Data(format!(
                "{file}: nothing names this game and it carries no minted gamedb id"
            )));
        };
        if let Some(other) = game_owner.insert(id.clone(), file.to_string()) {
            return Err(Error::Data(format!(
                "'{id}' is the canonical id of both {other} and {file}"
            )));
        }

        games.push(Game {
            id: id.clone(),
            title: page.str_field("title").unwrap_or_default().to_string(),
            year: page.int_field("year"),
            variant_of: page.str_field("variant_of").map(str::to_string),
            note: page.str_field("note").map(str::to_string),
            steam: page.id_field("steam").and_then(toml::Value::as_integer),
            umu: page
                .id_field("umu")
                .and_then(toml::Value::as_str)
                .map(str::to_string),
        });

        // Every identifier that resolves to this page. Wider than the set the
        // lint guards: the lint takes store codenames only from the stores
        // that can lend an id, while a lookup has to resolve a `[[stores.umu]]`
        // or `[[stores.steam]]` codename too.
        let mut claimed: Vec<String> = vec![id.clone()];
        if let Some(steam) = page.id_field("steam") {
            claimed.push(format!("steam-{}", scalar(steam)));
        }
        if let Some(umu) = page.id_field("umu").and_then(toml::Value::as_str) {
            claimed.push(umu.to_string());
        }
        for absorbed in page.merged_from() {
            claimed.push(absorbed.to_string());
        }
        for exe in page.exes() {
            claimed.push(format!("exe:{}", exe_basename(exe)));
        }

        for (store, entries) in page.stores() {
            for entry in entries {
                let field = |key: &str| entry.get(key).and_then(toml::Value::as_str);
                let Some(codename) = field("codename") else {
                    return Err(Error::Data(format!(
                        "{file}: a {store} entry has no codename"
                    )));
                };
                claimed.push(format!("{store}-{codename}"));
                if let Some(exe) = field("exe") {
                    claimed.push(format!("exe:{}", exe_basename(exe)));
                }
                stores.push(StoreRow {
                    id: id.clone(),
                    store: store.to_string(),
                    codename: codename.to_string(),
                    edition: field("edition").map(str::to_string),
                    exe: field("exe").map(str::to_string),
                    seen: field("seen").unwrap_or_default().to_string(),
                    source: field("source").unwrap_or_default().to_string(),
                    confidence: field("confidence").unwrap_or_default().to_string(),
                });
            }
        }

        for alias in claimed {
            match alias_owner.get(&alias) {
                // The same page naming one identifier twice is ordinary: a
                // GOG page whose canonical id *is* its store codename.
                Some(owner) if owner == &id => continue,
                Some(owner) => {
                    return Err(Error::Data(format!(
                        "'{alias}' resolves to both '{owner}' and '{id}' - \
                         the same game cannot live on two pages"
                    )))
                }
                None => {}
            }
            alias_owner.insert(alias.clone(), id.clone());
            aliases.push(Alias {
                alias,
                id: id.clone(),
            });
        }
    }

    let mut helpers: Vec<Helper> = Vec::new();
    let mut helper_seen: HashMap<String, ()> = HashMap::new();
    for entry in data.helper_entries() {
        let field = |key: &str| entry.get(key).and_then(toml::Value::as_str);
        let Some(exe) = field("exe") else {
            return Err(Error::Data(format!(
                "{}: a helper entry has no exe",
                data.helpers_path.display()
            )));
        };
        // Consumers match case-insensitively, so lowercase is the canonical
        // form and two spellings of one basename are one entry, not two.
        let exe = exe.to_lowercase();
        if helper_seen.insert(exe.clone(), ()).is_some() {
            return Err(Error::Data(format!(
                "{}: '{exe}' is listed twice",
                data.helpers_path.display()
            )));
        }
        helpers.push(Helper {
            exe,
            reason: field("reason").unwrap_or_default().to_string(),
            incident: field("incident").map(str::to_string),
            seen: field("seen").unwrap_or_default().to_string(),
        });
    }

    games.sort_by(|a, b| a.id.cmp(&b.id));
    stores.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.store.cmp(&b.store))
            .then_with(|| a.codename.cmp(&b.codename))
    });
    aliases.sort_by(|a, b| a.alias.cmp(&b.alias));
    helpers.sort_by(|a, b| a.exe.cmp(&b.exe));

    Ok(Tables {
        schema_version: crate::SCHEMA_VERSION,
        games,
        stores,
        aliases,
        helpers,
    })
}

/// Read a data set and fold it, without writing anything. For tests and for
/// anyone wanting the tables in memory.
pub fn tables_from(data_dir: &Path) -> Result<Tables> {
    tables(&DataSet::load(data_dir, None)?)
}
