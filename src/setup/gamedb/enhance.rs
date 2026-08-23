//! Adding what this machine knows to a page gamebus-gamedb already has.
//!
//! The emitter in `pages.rs` writes a whole new page and owns every byte of
//! it. This does the opposite: it adds to somebody else's file - a
//! reviewer's note, a `merged_from` list, a comment explaining a decision -
//! and everything it does not add has to come out exactly as it went in, so
//! the pull request shows two lines of diff and not a reformatted file.
//! Hence `toml_edit`, which remembers a document's own formatting.
//!
//! Every operation below is an insert. Nothing is removed, nothing is
//! reordered, and an addition the page already carries is skipped.

use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

use super::pages::{Addition, StoreEntry};

/// Apply every addition to a page's own text, and hand back the new text.
/// An unparseable page is an error rather than a silent rewrite: it is not
/// ours to guess at.
pub(super) fn apply(text: &str, additions: &[Addition]) -> Result<String, String> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("not readable as a gamebus-gamedb page: {e}"))?;
    for addition in additions {
        match addition {
            Addition::Store(entry) => add_store(&mut doc, entry),
            Addition::Exe(exe) => add_exe(&mut doc, exe),
            Addition::Steam { appid, seen } => add_steam(&mut doc, *appid, seen),
        }
    }
    Ok(doc.to_string())
}

/// Append `[[stores.<store>]]`, in the field order every page in the data
/// set uses. `stores` itself is an implicit table: the format nests the
/// arrays under it and never writes a bare `[stores]` header.
fn add_store(doc: &mut DocumentMut, entry: &StoreEntry) {
    if doc.get("stores").is_none() {
        let mut table = Table::new();
        table.set_implicit(true);
        doc.insert("stores", Item::Table(table));
    }
    let Some(stores) = doc.get_mut("stores").and_then(Item::as_table_mut) else {
        return;
    };
    if stores.get(&entry.store).is_none() {
        stores.insert(&entry.store, Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let Some(array) = stores
        .get_mut(&entry.store)
        .and_then(Item::as_array_of_tables_mut)
    else {
        return;
    };
    // Already listed - a second export, or an index older than the page it
    // points at. Writing it twice is a lint failure.
    if array.iter().any(|existing| {
        existing.get("codename").and_then(|c| c.as_str()) == Some(entry.codename.as_str())
    }) {
        return;
    }
    let mut table = Table::new();
    table.insert("codename", value(entry.codename.as_str()));
    if let Some(exe) = &entry.exe {
        table.insert("exe", value(exe.as_str()));
    }
    table.insert("seen", value(entry.seen.as_str()));
    table.insert("source", value(entry.source.as_str()));
    table.insert("confidence", value(entry.confidence.as_str()));
    array.push(table);
}

/// Add an executable to the page-level `exe` list, creating it when the page
/// has none.
///
/// It has to be created at the ROOT: a bare key written after a table header
/// belongs to that table, so an `exe` under `[ids]` would parse as `ids.exe`
/// and the schema would reject the page. Inserting into the document's root
/// table is what makes that impossible - toml_edit writes a table's own
/// key-values before any sub-table header, however late they were added.
fn add_exe(doc: &mut DocumentMut, exe: &str) {
    if doc.get("exe").is_none() {
        doc.insert("exe", value(Array::new()));
    }
    let Some(array) = doc.get_mut("exe").and_then(Item::as_array_mut) else {
        return;
    };
    // Consumers match executables case-insensitively, so a differently
    // capitalized spelling of one already there is the same entry.
    if array
        .iter()
        .any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(exe)))
    {
        return;
    }
    array.push(exe);
}

/// Record the Steam app id in `[ids]`.
///
/// An existing `[ids]` gets the one key and keeps its own `source` and
/// `seen` - those describe how its other identifiers were learned, and
/// rewriting them would restate somebody else's claim. A page with no
/// `[ids]` gets the whole table, sourced as `steam-sku`: the only basis the
/// stash accepts an app id from.
fn add_steam(doc: &mut DocumentMut, appid: u64, seen: &str) {
    if let Some(item) = doc.get_mut("ids") {
        if let Some(ids) = item.as_table_like_mut() {
            if ids.get("steam").is_none() {
                ids.insert("steam", value(appid as i64));
            }
        }
        return;
    }
    let mut ids = Table::new();
    ids.insert("steam", value(appid as i64));
    ids.insert("source", value("steam-sku"));
    ids.insert("seen", value(seen));
    doc.insert("ids", Item::Table(ids));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `gamedb/games/control.toml` as published, byte for byte. The point of
    /// the golden below is that all of this survives untouched.
    const CONTROL: &str = "title = \"Control\"\n\
         gamedb = \"steam-870780\"\n\
         note = \"\"\"\n\
         No protonfix exists upstream for this game (checked 2026-08-22), so \\\n\
         umu-database does not want it and it therefore has no umu id. The Epic \\\n\
         codename below has nowhere else to go.\"\"\"\n\
         \n\
         [ids]\n\
         steam = 870780\n\
         source = \"steam-sku\"\n\
         seen = \"2026-08-22\"\n\
         \n\
         [[stores.egs]]\n\
         codename = \"Calluna\"\n\
         seen = \"2026-08-15\"\n\
         source = \"heroic-config\"\n\
         confidence = \"high\"\n\
         note = \"Heroic writes the app name capitalized; Lutris matches it case-sensitively.\"\n";

    fn gog_entry() -> StoreEntry {
        StoreEntry {
            store: "gog".into(),
            codename: "1660194629".into(),
            exe: None,
            seen: "2026-08-16".into(),
            source: "manual".into(),
            confidence: "high".into(),
        }
    }

    /// The whole promise in one assertion: the original bytes are still
    /// there, in their original order, and the additions are appended.
    #[test]
    fn the_published_page_survives_byte_for_byte_and_the_additions_are_appended() {
        let out = apply(
            CONTROL,
            &[
                Addition::Store(gog_entry()),
                Addition::Exe("Control_DX12.exe".into()),
            ],
        )
        .expect("the page applies");

        assert_eq!(
            out,
            "title = \"Control\"\n\
             gamedb = \"steam-870780\"\n\
             note = \"\"\"\n\
             No protonfix exists upstream for this game (checked 2026-08-22), so \\\n\
             umu-database does not want it and it therefore has no umu id. The Epic \\\n\
             codename below has nowhere else to go.\"\"\"\n\
             exe = [\"Control_DX12.exe\"]\n\
             \n\
             [ids]\n\
             steam = 870780\n\
             source = \"steam-sku\"\n\
             seen = \"2026-08-22\"\n\
             \n\
             [[stores.egs]]\n\
             codename = \"Calluna\"\n\
             seen = \"2026-08-15\"\n\
             source = \"heroic-config\"\n\
             confidence = \"high\"\n\
             note = \"Heroic writes the app name capitalized; Lutris matches it case-sensitively.\"\n\
             \n\
             [[stores.gog]]\n\
             codename = \"1660194629\"\n\
             seen = \"2026-08-16\"\n\
             source = \"manual\"\n\
             confidence = \"high\"\n"
        );
    }

    /// The failure this guards against: `exe` written after a table header
    /// parses as that table's key. The published page opens with `[ids]`, so
    /// an `exe` appended naively would become `ids.exe` and the schema would
    /// reject the page.
    #[test]
    fn a_created_exe_list_is_a_root_key_and_never_lands_inside_ids() {
        let out =
            apply(CONTROL, &[Addition::Exe("Control_DX12.exe".into())]).expect("the page applies");
        let doc: DocumentMut = out.parse().expect("the result is still valid TOML");
        assert!(
            doc.get("exe").and_then(Item::as_array).is_some(),
            "exe is not a root array:\n{out}"
        );
        assert!(
            doc["ids"].get("exe").is_none(),
            "exe landed under [ids]:\n{out}"
        );
        // And textually: above the first table header, which is what makes
        // it a root key rather than a member of one.
        let exe_at = out.find("exe = [").expect("the list was written");
        let first_header = out.find("\n[").expect("the page has a table");
        assert!(exe_at < first_header, "exe is below a table header:\n{out}");
    }

    /// A page with no `[ids]` at all gets the whole table, and one that has
    /// an `[ids]` without `steam` gets the single key - never a rewritten
    /// `source` or `seen`.
    #[test]
    fn a_steam_app_id_creates_the_ids_table_or_joins_the_one_there() {
        let bare = "title = \"Fixture Quest\"\n\
                    \n\
                    [[stores.egs]]\n\
                    codename = \"Catnip\"\n\
                    seen = \"2026-08-07\"\n\
                    source = \"heroic-config\"\n\
                    confidence = \"high\"\n";
        let out = apply(
            bare,
            &[Addition::Steam {
                appid: 870780,
                seen: "2026-08-15".into(),
            }],
        )
        .expect("applies");
        assert!(
            out.contains("[ids]\nsteam = 870780\nsource = \"steam-sku\"\nseen = \"2026-08-15\"\n"),
            "{out}"
        );

        let with_ids =
            "title = \"X\"\n\n[ids]\ngog = \"12\"\nsource = \"store-id\"\nseen = \"2026-08-01\"\n";
        let out = apply(
            with_ids,
            &[Addition::Steam {
                appid: 42,
                seen: "2026-08-20".into(),
            }],
        )
        .expect("applies");
        assert!(out.contains("gog = \"12\""), "{out}");
        assert!(
            out.contains("source = \"store-id\""),
            "the source was rewritten:\n{out}"
        );
        assert!(
            out.contains("seen = \"2026-08-01\""),
            "the date was rewritten:\n{out}"
        );
        assert!(out.contains("steam = 42"), "{out}");
    }

    /// Applying the same addition twice must not write it twice: the second
    /// export of an unchanged stash is a no-op, not a duplicate entry.
    #[test]
    fn an_executable_the_page_already_names_is_not_written_again() {
        let once = apply(CONTROL, &[Addition::Exe("Control_DX12.exe".into())]).expect("applies");
        let twice = apply(&once, &[Addition::Exe("control_dx12.EXE".into())]).expect("applies");
        assert_eq!(once, twice, "a differently cased spelling was added again");
    }

    /// The same guard on the store side. A second export against an index
    /// that has not caught up yet must not append the entry twice.
    #[test]
    fn a_store_entry_the_page_already_lists_is_not_written_again() {
        let once = apply(CONTROL, &[Addition::Store(gog_entry())]).expect("applies");
        let twice = apply(&once, &[Addition::Store(gog_entry())]).expect("applies");
        assert_eq!(once, twice, "the store entry was appended twice");
    }

    #[test]
    fn a_page_that_is_not_toml_is_refused_rather_than_rewritten() {
        let e = apply("<!doctype html>", &[Addition::Exe("X.exe".into())])
            .expect_err("HTML is not a page");
        assert!(e.contains("not readable as a gamebus-gamedb page"), "{e}");
    }
}
