//! Restart-survival cache.
//!
//! When the daemon restarts, connected games get an EOF and whether they
//! reconnect is the client's choice - many never retry. The cache makes a
//! restart usually seamless anyway: the published records are written to
//! `$XDG_RUNTIME_DIR/gamebus-presenced/cache.json` on every change, keyed by
//! pid plus the process start-time from `/proc/<pid>/stat` (field 22). On
//! startup, a record is re-adopted only if its process is still alive AND
//! the start-time still matches - the start-time is what stops pid reuse
//! from resurrecting a dead game's presence onto a different process.
//!
//! The cache lives on tmpfs (`$XDG_RUNTIME_DIR`): it survives a daemon
//! restart and dies at logout, which is exactly the lifetime of the
//! processes it describes. Re-adopted records are best-effort by
//! construction (design doc): sources that re-derive (GameMode) overwrite
//! them; fields whose source never comes back simply stay as last known.

use crate::dbus::types::Activity;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Cache format version, for future migrations.
const CACHE_VERSION: u32 = 1;

/// One cached record: the published activity plus the identity proof of the
/// process it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedRecord {
    pub pid: u32,
    /// `/proc/<pid>/stat` field 22 (process start-time in clock ticks since
    /// boot) at cache-write time. Re-adoption requires an exact match.
    pub start_time: u64,
    pub activity: Activity,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    records: Vec<CachedRecord>,
}

/// The cache file path within a runtime directory.
pub fn cache_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("gamebus-presenced").join("cache.json")
}

/// Process start-time (field 22 of `/proc/<pid>/stat`), or None if the
/// process is gone or unreadable.
///
/// The comm field (field 2) is parenthesised and may itself contain spaces
/// and parens, so parsing starts after the LAST ')' on the line; the tokens
/// that follow are fields 3, 4, ... making starttime token index 22 - 3 = 19.
pub fn process_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_start_time(&stat)
}

fn parse_start_time(stat: &str) -> Option<u64> {
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

/// Persist the records atomically (write-then-rename). Failures are logged,
/// never fatal: the cache is best-effort by design.
pub fn save_in(dir: &Path, records: &[CachedRecord]) {
    let path = cache_path(dir);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!(error = %e, "Failed to create cache directory");
            return;
        }
    }
    let file = CacheFile {
        version: CACHE_VERSION,
        records: records.to_vec(),
    };
    let payload = match serde_json::to_vec(&file) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Failed to serialise activity cache");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, payload) {
        warn!(error = %e, "Failed to write activity cache");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        warn!(error = %e, "Failed to commit activity cache");
    }
}

/// Load records whose process is still alive AND still the same process
/// (start-time match). A missing or corrupt cache is an empty cache.
pub fn load_in(dir: &Path) -> Vec<CachedRecord> {
    let path = cache_path(dir);
    let payload = match std::fs::read(&path) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            warn!(error = %e, path = %path.display(), "Failed to read activity cache");
            return Vec::new();
        }
    };
    let file: CacheFile = match serde_json::from_slice(&payload) {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, path = %path.display(), "Ignoring corrupt activity cache");
            return Vec::new();
        }
    };
    if file.version != CACHE_VERSION {
        info!(
            found = file.version,
            expected = CACHE_VERSION,
            "Ignoring cache from a different format version"
        );
        return Vec::new();
    }

    let mut adopted = Vec::new();
    for record in file.records {
        match process_start_time(record.pid) {
            Some(start) if start == record.start_time => adopted.push(record),
            Some(_) => debug!(
                pid = record.pid,
                "Cache entry rejected: pid reused by a different process"
            ),
            None => debug!(pid = record.pid, "Cache entry rejected: process gone"),
        }
    }
    adopted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus::types::Source;

    fn record(pid: u32, start_time: u64) -> CachedRecord {
        CachedRecord {
            pid,
            start_time,
            activity: Activity::from_gamemode(pid as i32, "/games/testgame", 1_700_000_000),
        }
    }

    #[test]
    fn parses_start_time_field_22() {
        // Realistic stat line; comm contains a space and a paren on purpose.
        // Fields after comm: state(3) ppid pgrp session tty_nr tpgid flags
        // minflt cminflt majflt cmajflt utime stime cutime cstime priority
        // nice num_threads itrealvalue starttime(22) ...
        let stat = "1234 (my game) S 1000 1000 1000 0 -1 4194304 500 0 0 0 10 5 0 0 20 0 1 0 987654321 12345678 999 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        assert_eq!(parse_start_time(stat), Some(987654321));
    }

    #[test]
    fn reads_own_start_time_from_proc() {
        let pid = std::process::id();
        let start = process_start_time(pid).expect("own stat must be readable");
        assert!(start > 0);
        // Reading twice is stable within a process lifetime.
        assert_eq!(process_start_time(pid), Some(start));
    }

    #[test]
    fn round_trip_re_adopts_live_process() {
        let dir = std::env::temp_dir().join(format!("gamebus-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let pid = std::process::id();
        let start = process_start_time(pid).unwrap();
        save_in(&dir, &[record(pid, start)]);

        let loaded = load_in(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].pid, pid);
        assert_eq!(loaded[0].activity.id, format!("pid_{pid}"));
        assert_eq!(loaded[0].activity.sources, vec![Source::GameMode]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pid_reuse_guard_rejects_start_time_mismatch() {
        let dir =
            std::env::temp_dir().join(format!("gamebus-cache-test-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let pid = std::process::id();
        let real_start = process_start_time(pid).unwrap();
        // Same live pid, wrong start-time: must be rejected (this is the
        // pid-reuse case - a different process wearing a cached pid).
        save_in(&dir, &[record(pid, real_start + 1)]);
        assert!(load_in(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dead_process_is_not_re_adopted() {
        let dir =
            std::env::temp_dir().join(format!("gamebus-cache-test-dead-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // pid 2^22 is almost certainly not a live process.
        save_in(&dir, &[record(4_194_304, 1)]);
        assert!(load_in(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_corrupt_cache_is_empty() {
        let dir =
            std::env::temp_dir().join(format!("gamebus-cache-test-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_in(&dir).is_empty());

        std::fs::create_dir_all(dir.join("gamebus-presenced")).unwrap();
        std::fs::write(cache_path(&dir), b"{not json").unwrap();
        assert!(load_in(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
