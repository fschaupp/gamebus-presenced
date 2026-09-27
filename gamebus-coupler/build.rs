//! Compile gamedb's `stores.toml` into the coupler.
//!
//! The store vocabulary is data in gamebus-gamedb, not a list in code. It is
//! read here at build time rather than at run time because the daemon
//! normalises store names on every launch and never touches the network or
//! the data set. A malformed file fails the build with the reason, rather
//! than producing a normaliser that quietly maps a store to the wrong id.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    id_precedence: Vec<String>,
    store: Vec<Store>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default = "yes")]
    pick: bool,
    #[serde(default)]
    standalone: bool,
}

fn yes() -> bool {
    true
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let path = manifest.join("../gamedb/stores.toml");
    println!("cargo:rerun-if-changed={}", path.display());
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}. The store list comes from the gamedb submodule: \
             `git submodule update --init gamedb`.",
            path.display()
        )
    });
    let file: File = toml::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if let Err(e) = check(&file) {
        panic!("{}: {e}", path.display());
    }

    let mut out = String::from("// Generated from gamedb/stores.toml by build.rs.\n\n");
    out.push_str("pub static STORES: &[Store] = &[\n");
    for s in &file.store {
        out.push_str(&format!(
            "    Store {{ id: {:?}, aliases: &{:?}, pick: {}, standalone: {} }},\n",
            s.id, s.aliases, s.pick, s.standalone
        ));
    }
    out.push_str("];\n\n");
    out.push_str(&format!(
        "pub static ID_PRECEDENCE: &[&str] = &{:?};\n",
        file.id_precedence
    ));
    let standalone = file.store.iter().find(|s| s.standalone).expect("checked");
    out.push_str(&format!(
        "pub const STANDALONE: &str = {:?};\n",
        standalone.id
    ));

    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo")).join("stores.rs");
    std::fs::write(&dest, out).expect("OUT_DIR is writable");
}

fn check(file: &File) -> Result<(), String> {
    let mut spellings = HashSet::new();
    for s in &file.store {
        for spelling in std::iter::once(&s.id).chain(&s.aliases) {
            if spelling != &spelling.to_ascii_lowercase() {
                return Err(format!("{spelling:?} is not lowercase"));
            }
            if !spellings.insert(spelling.as_str()) {
                return Err(format!("{spelling:?} names two stores"));
            }
        }
    }
    let ids: HashSet<&str> = file.store.iter().map(|s| s.id.as_str()).collect();
    for p in &file.id_precedence {
        if !ids.contains(p.as_str()) {
            return Err(format!("id_precedence names {p:?}, which is not a store"));
        }
    }
    match file.store.iter().filter(|s| s.standalone).count() {
        1 => Ok(()),
        n => Err(format!(
            "{n} standalone entries; exactly one says \"no store\""
        )),
    }
}
