//! The three text artifacts: `identities.json`, `shared-helpers.txt` and
//! `manifest.toml`.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::build::{Artifact, Tables};
use crate::model::{Error, Result};

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// `identities.json`: the four tables, flat, in the order the shape declares
/// them. Pretty-printed, because the point of the JSON artifact is that a
/// person or a shell script can read it without a library.
pub fn json(path: &Path, tables: &Tables) -> Result<()> {
    let mut text = serde_json::to_string_pretty(tables).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');
    write(path, text.as_bytes())
}

/// `shared-helpers.txt`: the flat list the daemon already union-merges, one
/// lowercase basename per line.
///
/// No header. `gamedb/README.md` calls this "the flat list, one basename per
/// line", and the reasons live in `helpers.toml` and travel into the
/// artifacts as the `helpers` table, with `reason` and `incident` as data
/// rather than as a comment nothing can query. A generated header would also
/// make this artifact impossible to compare against the file the daemon
/// bundles, which is exactly the check worth having.
pub fn shared_helpers(path: &Path, tables: &Tables) -> Result<()> {
    let mut text = String::new();
    for helper in &tables.helpers {
        text.push_str(&helper.exe);
        text.push('\n');
    }
    write(path, text.as_bytes())
}

/// Hash one written artifact.
pub fn checksum(dir: &Path, name: &str) -> Result<Artifact> {
    let path = dir.join(name);
    let bytes = std::fs::read(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    Ok(Artifact {
        name: name.to_string(),
        bytes: bytes.len() as u64,
        sha256: hex(&Sha256::digest(&bytes)),
    })
}

/// `manifest.toml`: where the data came from and what was built from it.
///
/// The only artifact that carries the source commit. Everything else is a
/// pure function of the data, so a rebuild of the same pages produces the
/// same bytes and a checksum here is worth comparing.
pub fn manifest(path: &Path, commit: &str, artifacts: &[Artifact]) -> Result<()> {
    let mut text = String::new();
    text.push_str(
        "# What this build was made from, and what it produced.\n\
         #\n\
         # The artifacts themselves carry no commit and no timestamp - they are a\n\
         # function of the data alone, so the same pages always build to the same\n\
         # bytes. This file is the one that moves, which is what makes the\n\
         # checksums below worth comparing.\n\n",
    );
    text.push_str(&format!("schema_version = {}\n", crate::SCHEMA_VERSION));
    text.push_str(&format!("source_commit = {}\n", toml_string(commit)));
    for artifact in artifacts {
        text.push_str("\n[[artifact]]\n");
        text.push_str(&format!("name = {}\n", toml_string(&artifact.name)));
        text.push_str(&format!("bytes = {}\n", artifact.bytes));
        text.push_str(&format!("sha256 = {}\n", toml_string(&artifact.sha256)));
    }
    write(path, text.as_bytes())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// A TOML basic string. Values here are file names and hex digests, but a
/// `--commit` comes from outside and gets escaped like anything else.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_parses_back_as_toml() {
        let artifacts = vec![Artifact {
            name: "identities.json".into(),
            bytes: 12,
            sha256: "ab".repeat(32),
        }];
        let dir = std::env::temp_dir().join(format!("gamedb-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("manifest.toml");
        manifest(&path, "deadbeef", &artifacts).expect("write");

        let parsed: toml::Value = std::fs::read_to_string(&path)
            .expect("read")
            .parse()
            .expect("manifest is valid TOML");
        assert_eq!(parsed["source_commit"].as_str(), Some("deadbeef"));
        assert_eq!(parsed["artifact"][0]["bytes"].as_integer(), Some(12));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_commit_with_quotes_in_it_cannot_break_the_manifest() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
