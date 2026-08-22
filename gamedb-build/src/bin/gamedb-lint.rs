//! Validate a gamedb data set: schemas, ids, and the one-game-one-page rule.
//!
//! Usage: gamedb-lint [directory] [--schema <dir>]   (default: gamedb)
//!
//! The Rust half of a pair. `.scripts/gamedb-lint.py` is the same rules in
//! stdlib Python, kept so a data-only repository can gate a merge on an
//! interpreter alone, and the two are held to one fixture set. Same report,
//! same exit status: 1 if anything failed, so CI can gate a merge on it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut base: Option<PathBuf> = None;
    let mut schema: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: gamedb-lint [directory] [--schema <dir>]\n\n\
                     Validates a gamedb data set: both JSON Schemas, the id rules, and\n\
                     the rule that every identifier resolves to exactly one page.\n\
                     Exits 1 if anything failed.\n\n\
                     directory   the data set (default: gamedb)\n\
                     --schema    where the schemas live (default: <directory>/schema,\n\
                     \x20           falling back to gamedb/schema)"
                );
                return ExitCode::SUCCESS;
            }
            "--schema" => match args.next() {
                Some(dir) => schema = Some(PathBuf::from(dir)),
                None => {
                    eprintln!("gamedb-lint: --schema needs a directory");
                    return ExitCode::from(2);
                }
            },
            other if other.starts_with('-') => {
                eprintln!("gamedb-lint: unknown option {other}");
                return ExitCode::from(2);
            }
            other if base.is_none() => base = Some(PathBuf::from(other)),
            other => {
                eprintln!("gamedb-lint: unexpected argument {other}");
                return ExitCode::from(2);
            }
        }
    }

    let base = base.unwrap_or_else(|| PathBuf::from("gamedb"));
    match gamedb_build::lint::run(&base, schema.as_deref().map(Path::new)) {
        Ok(report) => {
            print!("{}", report.render());
            if report.ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("gamedb-lint: {e}");
            ExitCode::from(2)
        }
    }
}
