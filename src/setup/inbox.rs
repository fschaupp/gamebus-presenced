//! The drop directory beisl writes compat findings into.
//!
//! `compat --record` is a synchronous handover: beisl execs this tool and
//! waits for it. That is the wrong shape for a detector. Two of the four
//! walls exist only on the launch that broke, so the capture has to happen
//! the moment the wall is seen - and at that moment gamebus-setup may not be
//! installed, may not be on PATH, and two detections may land at once, in
//! which case two processes read-modify-write one stash file and one of them
//! loses its finding.
//!
//! So the trigger writes a file instead. One delivery per file, into
//! `$XDG_DATA_HOME/gamebus-presenced/compat-inbox/`, and beisl is done: no
//! gamebus binary, no bus round trip, nothing to wait for and nothing to fail
//! at detection time. gamebus-setup drains the directory on its next run,
//! which keeps this tool the stash's only writer.
//!
//! The writer's half of the contract, which beisl implements:
//!
//! 1. Write the payload under a name that does NOT end in `.json` (use
//!    `<name>.json.tmp`), then rename it onto `<name>.json` in the same
//!    directory. This side only ever opens `*.json`, so a half-written file
//!    is invisible to it and the rename is what publishes the drop.
//! 2. Name it so a lexical sort is arrival order - `<unix-millis>-<run>.json`
//!    - because that is the order this side ingests in.
//! 3. Never delete or rewrite a published drop. Once renamed it belongs to
//!    this side, which removes it only after it is safely in the stash.
//!
//! The payload is exactly what `--record` takes: one delivery object, or an
//! array of them. `gamebus-setup compat --inbox` prints the directory and
//! creates it, so beisl can ask for the path rather than rebuild the XDG
//! derivation and drift from it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Refuse a drop larger than this. A finding is a few hundred bytes; anything
/// near a megabyte is a mistake, and it is the reader that pays for it.
const MAX_DROP_BYTES: u64 = 1 << 20;

/// Where drops land. `None` only when neither XDG_DATA_HOME nor HOME is set.
pub fn dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("gamebus-presenced").join("compat-inbox"))
}

pub fn ensure(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// A published drop is a plain file whose name ends in `.json`. Everything
/// else in the directory is someone's business but not ours: `.json.tmp` is a
/// write in progress, `.rejected` is a drop this side already refused.
pub fn is_drop(name: &str) -> bool {
    name.ends_with(".json") && !name.starts_with('.')
}

/// Published drops, in the order they should be ingested.
///
/// Sorted by name, which is arrival order when the writer follows the naming
/// rule and is at least deterministic when it does not. A missing directory
/// is not an error - it means beisl has never reported.
pub fn pending(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut drops: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter(|e| e.file_name().to_str().is_some_and(is_drop))
        .map(|e| e.path())
        .collect();
    drops.sort();
    drops
}

pub fn read(path: &Path) -> Result<String, String> {
    let len = std::fs::metadata(path)
        .map_err(|e| format!("cannot stat: {e}"))?
        .len();
    if len > MAX_DROP_BYTES {
        return Err(format!("{len} bytes, over the {MAX_DROP_BYTES} byte limit"));
    }
    std::fs::read_to_string(path).map_err(|e| format!("cannot read: {e}"))
}

/// Park a drop this side refused, next to itself.
///
/// Kept rather than deleted: a payload we could not parse is still the only
/// record that beisl saw something, and the writer is entitled to find it and
/// see what it got wrong. Renamed rather than left in place so the next run
/// does not retry it forever and report the same failure again.
///
/// Returns where it landed, or `None` if even the rename failed - in which
/// case the drop stays put and will be refused again, which is noisy but
/// loses nothing.
pub fn reject(path: &Path) -> Option<PathBuf> {
    let mut name: OsString = path.file_name()?.to_os_string();
    name.push(".rejected");
    let landed = path.with_file_name(name);
    std::fs::rename(path, &landed).ok()?;
    Some(landed)
}

/// Drop a file this side has taken. Failure is not worth reporting: the drop
/// is already in the stash, and a re-ingest of the same observation is a
/// no-op.
pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gamebus-inbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ensure(&dir).unwrap();
        dir
    }

    #[test]
    fn only_published_drops_are_pending() {
        assert!(is_drop("1730000000000-run-42.json"));
        // The writer's own in-progress name, and our own parked failure.
        assert!(!is_drop("1730000000000-run-42.json.tmp"));
        assert!(!is_drop("1730000000000-run-42.json.rejected"));
        assert!(!is_drop(".hidden.json"));
        assert!(!is_drop("notes.txt"));
    }

    #[test]
    fn pending_is_sorted_and_skips_partial_writes() {
        let dir = scratch("sorted");
        for name in ["0002-b.json", "0001-a.json", "0003-c.json.tmp"] {
            std::fs::write(dir.join(name), "{}").unwrap();
        }
        std::fs::create_dir(dir.join("0000-dir.json")).unwrap();

        let names: Vec<String> = pending(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["0001-a.json", "0002-b.json"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_inbox_is_empty_not_an_error() {
        let dir = std::env::temp_dir().join("gamebus-inbox-absent-does-not-exist");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(pending(&dir).is_empty());
    }

    #[test]
    fn an_oversized_drop_is_refused_unread() {
        let dir = scratch("oversized");
        let path = dir.join("big.json");
        std::fs::write(&path, vec![b'x'; (MAX_DROP_BYTES + 1) as usize]).unwrap();

        let err = read(&path).unwrap_err();
        assert!(err.contains("over the"), "{err}");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rejected_drop_is_parked_not_deleted() {
        let dir = scratch("rejected");
        let path = dir.join("bad.json");
        std::fs::write(&path, "not json").unwrap();

        let landed = reject(&path).unwrap();
        assert_eq!(landed, dir.join("bad.json.rejected"));
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&landed).unwrap(), "not json");
        // And it is not picked up again.
        assert!(pending(&dir).is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
