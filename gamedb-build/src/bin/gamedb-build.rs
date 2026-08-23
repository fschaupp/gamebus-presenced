//! Build the published gamedb artifacts from the TOML pages.
//!
//! Usage: gamedb-build --data gamedb --out build/ [--commit <sha>]
//!
//! Writes `identities.json`, `identities.parquet`, `identities.sqlite`,
//! `shared-helpers.txt` and `manifest.toml`. The data set is linted first:
//! publishing a condensed form of data that would not pass the merge gate
//! would ship a contradiction.
//!
//! The commit recorded in the manifest comes from `--commit`, then
//! `GAMEDB_COMMIT`, and is `"unknown"` when neither says. It is the only
//! provenance in the output - no artifact carries a timestamp, so the same
//! pages always build to the same bytes.

use std::path::PathBuf;
use std::process::ExitCode;

use gamedb_build::build::{self, Options};

const USAGE: &str = "\
usage: gamedb-build [--data <dir>] [--out <dir>] [--commit <sha>] [--no-lint]

  --data <dir>    the data set: games/, helpers.toml, schema/ (default: gamedb)
  --out <dir>     where the artifacts go, created if missing (default: build)
  --commit <sha>  the commit the data came from, for manifest.toml. Falls back
                  to $GAMEDB_COMMIT, then to \"unknown\".
  --no-lint       build without gating on the lint first. For inspecting a set
                  you already know is broken; never for publishing.
  --schema <dir>  where the JSON Schemas live (default: <data>/schema)
";

fn main() -> ExitCode {
    let mut data = PathBuf::from("gamedb");
    let mut out = PathBuf::from("build");
    let mut commit: Option<String> = None;
    let mut skip_lint = false;
    let mut schema_dir: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| match args.next() {
            Some(v) => Ok(v),
            None => Err(format!("{name} needs an argument")),
        };
        let parsed = match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--data" => value("--data").map(|v| data = PathBuf::from(v)),
            "--out" => value("--out").map(|v| out = PathBuf::from(v)),
            "--commit" => value("--commit").map(|v| commit = Some(v)),
            "--schema" => value("--schema").map(|v| schema_dir = Some(PathBuf::from(v))),
            "--no-lint" => {
                skip_lint = true;
                Ok(())
            }
            other => Err(format!("unknown argument {other}")),
        };
        if let Err(message) = parsed {
            eprintln!("gamedb-build: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    }

    let commit = commit
        .or_else(|| std::env::var("GAMEDB_COMMIT").ok())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let options = Options {
        data,
        out,
        commit,
        skip_lint,
        schema_dir,
    };

    match build::run(&options) {
        Ok(outcome) => {
            for warning in &outcome.warnings {
                eprintln!("warning: {warning}");
            }
            println!(
                "{} games, {} store entries, {} aliases, {} helpers from {}",
                outcome.tables.games.len(),
                outcome.tables.stores.len(),
                outcome.tables.aliases.len(),
                outcome.tables.helpers.len(),
                options.data.display()
            );
            for artifact in &outcome.artifacts {
                println!(
                    "  {:<20} {:>9} bytes  {}",
                    artifact.name, artifact.bytes, artifact.sha256
                );
            }
            println!(
                "  {:<20} {:>9}         source commit {}",
                build::MANIFEST,
                "",
                options.commit
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("gamedb-build: {e}");
            ExitCode::FAILURE
        }
    }
}
