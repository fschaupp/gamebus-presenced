//! The merge gate: schemas, ids, and the one-game-one-page rule.
//!
//! A port of `.scripts/gamedb-lint.py`, held to that script's own fixture set
//! (`.scripts/gamedb-lint-fixtures/`, one page per rule plus pages that must
//! stay silent) by `tests/lint_fixtures.rs`, which compares the whole report
//! byte for byte against `expected.txt`. The two implementations are meant to
//! be interchangeable, so a rule changed here has to change there in the same
//! commit, and the fixture set is what proves it.
//!
//! Rules beyond the schemas, in the order the report emits them:
//!
//! * a page's canonical id is one its own identifiers justify, or a
//!   well-formed minted `gamedb-<uid>` carrying at least one letter
//! * an id is assigned once and frozen. A page that later learns a better
//!   identifier keeps the id it had, and that is a **warning**: re-pointing
//!   an id would retire an identifier something else already resolved
//! * every identifier a page carries - canonical id, `<store>-<codename>`,
//!   the Steam app id, the umu id, executables, and everything absorbed
//!   through `merged_from` - resolves to exactly one page
//! * absorbed ids stay reserved: never a live id elsewhere, never absorbed
//!   twice
//! * `variant_of` names a page that exists, live or retired
//! * a umu id appears only where the page says umu-database named it
//! * `ids.<store>` does not repeat a codename a store entry already carries
//! * a page identifies something: a store entry or an executable
//! * the file name is a lowercase slug of the title, optionally qualified
//!
//! The prefix of an id names its namespace and nothing parses the shape that
//! follows, which is why `gog-1660194629` is a perfectly ordinary canonical
//! id. There is no rule keyed on a numeric second part, and there must not
//! be: GOG product ids are always numeric, and such a rule would force every
//! GOG-only game into an opaque minted uid.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use regex::Regex;

use crate::jsonschema::Validator;
use crate::model::{exe_basename, scalar, DataSet, Result};

/// The whole report: what the run found, in the order it found it.
#[derive(Debug, Default)]
pub struct Report {
    /// Informational. An id that predates a better identifier is the only
    /// one today, and it never fails a merge.
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub pages: usize,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The report exactly as `.scripts/gamedb-lint.py` prints it, trailing
    /// newline included: warnings first, then either every error or the one
    /// line that says the set is valid.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.warnings.is_empty() {
            for warning in &self.warnings {
                out.push_str("warning: ");
                out.push_str(warning);
                out.push('\n');
            }
        }
        if self.errors.is_empty() {
            out.push_str(&format!("OK: {} pages + helpers valid\n", self.pages));
        } else {
            for error in &self.errors {
                out.push_str(error);
                out.push('\n');
            }
        }
        out
    }
}

/// Insertion-ordered set, which is what a Python `dict` used as one gives you
/// for free. The order decides which finding a page reports first, so it is
/// part of the contract with the fixture set, not an implementation detail.
#[derive(Default)]
struct OrderedSet {
    order: Vec<String>,
    seen: HashSet<String>,
}

impl OrderedSet {
    fn insert(&mut self, value: impl Into<String>) {
        let value = value.into();
        if self.seen.insert(value.clone()) {
            self.order.push(value);
        }
    }
}

struct Patterns {
    slug: Regex,
    not_slug: Regex,
    minted: Regex,
}

impl Patterns {
    fn new() -> Self {
        Self {
            slug: Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("literal"),
            not_slug: Regex::new(r"[^a-z0-9]+").expect("literal"),
            minted: Regex::new(r"^gamedb-[a-z0-9]*[a-z][a-z0-9]*$").expect("literal"),
        }
    }
}

/// Validate the data set at `base`.
///
/// `schema_dir` overrides where the schemas are found; `None` takes
/// `<base>/schema` when it exists and `gamedb/schema` relative to the working
/// directory otherwise, which is what the Python lint does.
pub fn run(base: &Path, schema_dir: Option<&Path>) -> Result<Report> {
    let data = DataSet::load(base, schema_dir)?;
    Ok(check(&data))
}

/// Validate an already-loaded data set.
pub fn check(data: &DataSet) -> Report {
    let mut report = Report {
        pages: data.pages.len(),
        ..Report::default()
    };
    let re = Patterns::new();

    // 1. Both schemas, applied to the parsed TOML.
    match schema_pass(data, &mut report.errors) {
        Ok(()) => {}
        Err(message) => {
            report.errors.push(message);
            return report;
        }
    }

    // 2. Ids, claims and the one-game-one-page rule, page by page.
    //
    // `ids_seen` maps a live canonical id to its page, `claims` maps every
    // identifier of any kind to the page that claimed it first. Both are
    // last-write-wins, as the Python dicts are: the collision has already
    // been reported by the time a second page overwrites an entry.
    let mut ids_seen: HashMap<String, String> = HashMap::new();
    let mut claims: HashMap<String, String> = HashMap::new();
    let mut retired: HashMap<String, String> = HashMap::new();
    let mut variants: Vec<(String, String)> = Vec::new();

    for page in &data.pages {
        let file = page.file.as_str();

        if !re.slug.is_match(&page.slug) {
            report
                .errors
                .push(format!("{file}: file name is not a lowercase slug"));
        }
        if let Some(title) = page.str_field("title") {
            let title_slug = re
                .not_slug
                .replace_all(&title.to_lowercase(), "-")
                .trim_matches('-')
                .to_string();
            if page.slug != title_slug && !page.slug.starts_with(&format!("{title_slug}-")) {
                report.errors.push(format!(
                    "{file}: file name is neither the title slug {} nor that slug plus a qualifier",
                    crate::pyrepr::string(&title_slug)
                ));
            }
        }

        let want = page.derived_id();
        let got = page.str_field("gamedb").map(str::to_string);
        let candidates = page.candidates();

        if let Some(got) = &got {
            let minted = re.minted.is_match(got);
            if got.starts_with("gamedb-") && !minted {
                report.errors.push(format!(
                    "{file}: minted id {} must be gamedb-<uid> and must contain a letter",
                    crate::pyrepr::string(got)
                ));
            } else if !candidates.contains(got) && !minted {
                report.errors.push(format!(
                    "{file}: gamedb is {}, which none of this page's identifiers justifies",
                    crate::pyrepr::string(got)
                ));
            } else if let Some(want) = &want {
                if got != want {
                    // Frozen on assignment. A better identifier turning up
                    // later is recorded in [ids] and changes nothing.
                    report.warnings.push(format!(
                        "{file}: id {} predates {}; kept, since ids are never re-pointed",
                        crate::pyrepr::string(got),
                        crate::pyrepr::string(want)
                    ));
                }
            }
        } else if want.is_none() {
            report.errors.push(format!(
                "{file}: nothing names this game, so it needs a minted gamedb id"
            ));
        }

        let key = got.clone().or_else(|| want.clone());
        if let Some(key) = &key {
            ids_seen.insert(key.clone(), file.to_string());
        }

        // Every identifier this page carries, in the order the report should
        // walk them.
        let mut claimed = OrderedSet::default();
        for candidate in &candidates {
            claimed.insert(candidate.clone());
        }
        if let Some(key) = &key {
            claimed.insert(key.clone());
        }
        for absorbed in page.merged_from() {
            claimed.insert(absorbed);
        }
        for exe in page.exes() {
            claimed.insert(format!("exe:{}", exe.to_lowercase()));
        }
        for (_, entries) in page.stores() {
            for entry in entries {
                if let Some(exe) = entry.get("exe").and_then(toml::Value::as_str) {
                    claimed.insert(format!("exe:{}", exe_basename(exe)));
                }
            }
        }

        for ident in &claimed.order {
            if let Some(owner) = claims.get(ident) {
                if owner != file {
                    let (a, b) = if owner.as_str() < file {
                        (owner.clone(), file.to_string())
                    } else {
                        (file.to_string(), owner.clone())
                    };
                    report.errors.push(format!(
                        "{} is claimed by both {a} and {b} - the same game cannot live on two \
                         pages, so consolidate them into one",
                        crate::pyrepr::string(ident)
                    ));
                }
            }
            claims.insert(ident.clone(), file.to_string());
        }

        // A umu id claims membership of umu-database. Only that source can
        // grant it: a game umu declined has no umu id at all.
        if page.id_field("umu").is_some()
            && page.id_field("source").and_then(toml::Value::as_str) != Some("umu-database")
        {
            report.errors.push(format!(
                "{file}: umu id present without an umu-database source"
            ));
        }

        // A fact is recorded once. A GOG product id *is* the GOG codename.
        for (store, entries) in page.stores() {
            for entry in entries {
                let Some(recorded) = page.id_field(store) else {
                    continue;
                };
                let Some(codename) = entry.get("codename").and_then(toml::Value::as_str) else {
                    continue;
                };
                if scalar(recorded) == codename {
                    report
                        .errors
                        .push(format!("{file}: ids.{store} repeats the {store} codename"));
                }
            }
        }

        if !page.has_stores() && page.exes().is_empty() {
            report.errors.push(format!(
                "{file}: names neither a store entry nor an executable, so it identifies nothing"
            ));
        }

        if let Some(target) = page.str_field("variant_of") {
            variants.push((file.to_string(), target.to_string()));
        }
    }

    // 3. Absorbed ids stay reserved forever.
    for page in &data.pages {
        let file = page.file.as_str();
        for absorbed in page.merged_from() {
            if let Some(owner) = ids_seen.get(absorbed) {
                if owner != file {
                    report.errors.push(format!(
                        "{file}: merged_from {} is the live id of {owner}",
                        crate::pyrepr::string(absorbed)
                    ));
                }
            }
            if let Some(earlier) = retired.get(absorbed) {
                if earlier != file {
                    report.errors.push(format!(
                        "{file}: merged_from {} was already absorbed by {earlier}",
                        crate::pyrepr::string(absorbed)
                    ));
                }
            }
            retired.insert(absorbed.to_string(), file.to_string());
        }
    }

    // 4. A variant points home to a page that exists, live or retired.
    for (file, target) in &variants {
        if !ids_seen.contains_key(target) && !retired.contains_key(target) {
            report.errors.push(format!(
                "{file}: variant_of {} names no page here",
                crate::pyrepr::string(target)
            ));
        }
    }

    report
}

/// Everything wrong with `stores.toml`: its schema, and its agreement with
/// the page schema. Shared by the lint and the builder, which runs it even
/// when told to skip the lint: no artifact is ever built from a store list
/// gamebus would refuse.
pub fn store_problems(data: &DataSet) -> std::result::Result<Vec<String>, String> {
    let game_schema = crate::model::read_json(&data.schema_dir.join("game.schema.json"))
        .map_err(|e| e.to_string())?;
    let stores_schema = crate::model::read_json(&data.schema_dir.join("stores.schema.json"))
        .map_err(|e| e.to_string())?;
    let mut errs = Vec::new();
    Validator::new(stores_schema).check(&data.stores, "stores.toml", &mut errs);
    store_keys_match(&game_schema, &data.stores, &mut errs);
    Ok(errs)
}

/// The store keys a page may use must be exactly the stores `stores.toml`
/// lists, less the standalone one; and the id precedence may only name those.
/// Also what the schema cannot say about the file, and gamebus's build
/// refuses: no spelling may name two stores, and exactly one entry is the
/// standalone "no store".
fn store_keys_match(game_schema: &serde_json::Value, stores: &toml::Value, errs: &mut Vec<String>) {
    use std::collections::BTreeSet;
    let entries = stores
        .get("store")
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let listed: BTreeSet<String> = entries
        .iter()
        .filter(|s| {
            !s.get("standalone")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|s| {
            s.get("id")
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let keys: BTreeSet<String> = game_schema
        .pointer("/properties/stores/propertyNames/enum")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    for entry in &entries {
        let id = entry.get("id").and_then(toml::Value::as_str);
        let aliases = entry.get("aliases").and_then(toml::Value::as_array);
        let spellings = id.into_iter().chain(
            aliases
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str),
        );
        for spelling in spellings {
            if !seen.insert(spelling.to_string()) {
                errs.push(format!("stores.toml: {spelling:?} names two stores"));
            }
        }
    }
    let standalone = entries
        .iter()
        .filter(|s| {
            s.get("standalone")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false)
        })
        .count();
    if standalone != 1 {
        errs.push(format!(
            "stores.toml: {standalone} standalone entries; exactly one says \"no store\""
        ));
    }

    let missing: Vec<&String> = listed.difference(&keys).collect();
    let extra: Vec<&String> = keys.difference(&listed).collect();
    if !missing.is_empty() || !extra.is_empty() {
        errs.push(format!(
            "game.schema.json: its store keys must match stores.toml (missing {missing:?}, not in stores.toml {extra:?})"
        ));
    }
    for p in stores
        .get("id_precedence")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
    {
        if !listed.contains(p) {
            errs.push(format!(
                "stores.toml: id_precedence names {p:?}, which is not a store it lists"
            ));
        }
    }
}

/// The schemas, against every page, `helpers.toml` and `stores.toml`; and
/// the page schema's store keys against `stores.toml`, the one list of
/// stores everything else is built from.
fn schema_pass(data: &DataSet, errs: &mut Vec<String>) -> std::result::Result<(), String> {
    let game_schema = crate::model::read_json(&data.schema_dir.join("game.schema.json"))
        .map_err(|e| e.to_string())?;
    let helpers_schema = crate::model::read_json(&data.schema_dir.join("helpers.schema.json"))
        .map_err(|e| e.to_string())?;
    errs.extend(store_problems(data)?);

    let mut games = Validator::new(game_schema);
    for page in &data.pages {
        games.check(&page.doc, &page.file, errs);
    }
    Validator::new(helpers_schema).check(&data.helpers, "helpers.toml", errs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::store_keys_match;

    fn stores() -> toml::Value {
        r#"
id_precedence = ["gog"]
[[store]]
id = "gog"
[[store]]
id = "steam"
[[store]]
id = "none"
standalone = true
"#
        .parse()
        .expect("fixture parses")
    }

    fn schema(keys: &[&str]) -> serde_json::Value {
        serde_json::json!({"properties": {"stores": {"propertyNames": {"enum": keys}}}})
    }

    #[test]
    fn the_page_schema_must_list_exactly_the_stores_file() {
        let mut errs = Vec::new();
        store_keys_match(&schema(&["gog", "steam"]), &stores(), &mut errs);
        assert!(errs.is_empty(), "{errs:?}");

        store_keys_match(&schema(&["gog", "steam", "origin"]), &stores(), &mut errs);
        store_keys_match(&schema(&["gog"]), &stores(), &mut errs);
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs[0].contains("\"origin\""), "{}", errs[0]);
        assert!(errs[1].contains("\"steam\""), "{}", errs[1]);
    }

    #[test]
    fn a_spelling_may_name_one_store_and_one_entry_is_no_store() {
        let bad: toml::Value = r#"
id_precedence = ["gog"]
[[store]]
id = "gog"
aliases = ["steam"]
[[store]]
id = "steam"
"#
        .parse()
        .expect("fixture parses");
        let mut errs = Vec::new();
        store_keys_match(&schema(&["gog", "steam"]), &bad, &mut errs);
        assert!(
            errs.iter()
                .any(|e| e.contains("\"steam\" names two stores")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|e| e.contains("0 standalone entries")),
            "{errs:?}"
        );
    }

    #[test]
    fn the_precedence_may_only_name_listed_stores() {
        let mut bad = stores();
        bad["id_precedence"] = toml::Value::Array(vec!["origin".into()]);
        let mut errs = Vec::new();
        store_keys_match(&schema(&["gog", "steam"]), &bad, &mut errs);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("origin"));
    }
}
