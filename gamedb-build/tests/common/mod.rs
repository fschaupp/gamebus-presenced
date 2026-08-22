//! Shared scaffolding: where the repo is, and a scratch directory that
//! cleans up after itself.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// The repository root: the workspace directory this crate sits in.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("gamedb-build sits inside the workspace")
        .to_path_buf()
}

/// The real data set the release builds from.
pub fn data_set() -> PathBuf {
    repo().join("gamedb")
}

/// The fixture set both linters are held to.
pub fn lint_fixtures() -> PathBuf {
    repo().join(".scripts/gamedb-lint-fixtures")
}

/// A data set written for this crate's own tests.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

pub fn schema_dir() -> PathBuf {
    data_set().join("schema")
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch directory, removed when it goes out of scope.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gamedb-build-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}
