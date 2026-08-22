//! The Rust lint against the Python lint's own fixture set.
//!
//! `.scripts/gamedb-lint-fixtures/` carries one page per rule plus pages that
//! must stay silent, and `expected.txt` is the whole report - warnings
//! included - that `.scripts/gamedb-lint-selftest.sh` holds the Python
//! implementation to. Holding this one to the same file is what makes the two
//! interchangeable rather than merely similar, and it means a rule cannot be
//! changed in one without the other failing.

mod common;

use gamedb_build::lint;

#[test]
fn the_report_matches_the_python_lint_byte_for_byte() {
    // The fixture set carries no schemas of its own; the Python lint falls
    // back to gamedb/schema relative to the working directory, which under
    // `cargo test` is the package rather than the repo.
    let report = lint::run(&common::lint_fixtures(), Some(&common::schema_dir()))
        .expect("the fixture set loads");
    let expected = std::fs::read_to_string(common::lint_fixtures().join("expected.txt"))
        .expect("expected.txt is readable");

    assert_eq!(
        report.render(),
        expected,
        "the Rust lint's report drifted from .scripts/gamedb-lint-fixtures/expected.txt.\n\
         Both linters are held to that file: change a rule in one and you change it in \
         the other, in the same commit."
    );
    assert!(!report.ok(), "the fixture set is meant to fail");
}

#[test]
fn every_fixture_rule_is_actually_exercised() {
    // A guard against the fixture set quietly losing a case: expected.txt has
    // to keep naming each rule the lint enforces.
    let expected = std::fs::read_to_string(common::lint_fixtures().join("expected.txt"))
        .expect("expected.txt is readable");
    for fragment in [
        "must be gamedb-<uid> and must contain a letter",
        "which none of this page's identifiers justifies",
        "predates",
        "the same game cannot live on two pages",
        "names neither a store entry nor an executable",
        "is the live id of",
    ] {
        assert!(
            expected.contains(fragment),
            "the fixture set no longer covers {fragment:?}"
        );
    }
}

#[test]
fn the_real_data_set_validates() {
    let report = lint::run(&common::data_set(), None).expect("the data set loads");
    assert!(
        report.ok(),
        "gamedb/ does not validate:\n{}",
        report.render()
    );
    assert_eq!(
        report.render(),
        format!("OK: {} pages + helpers valid\n", report.pages)
    );
}

#[test]
fn a_numeric_store_codename_is_a_perfectly_good_canonical_id() {
    // The fixture set carries prey.toml with gamedb = "gog-1207658770" and it
    // is silent about it. GOG product ids are always numeric, so a rule that
    // read a trailing number as a Steam app id would force every GOG-only
    // game into an opaque minted uid. The prefix names the namespace; nothing
    // parses the shape that follows.
    let expected = std::fs::read_to_string(common::lint_fixtures().join("expected.txt"))
        .expect("expected.txt is readable");
    assert!(!expected.contains("prey.toml"));
    assert!(!expected.contains("gog-1207658770"));
}
