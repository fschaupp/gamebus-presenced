//! `setup.toml` - the setup tool's own settings.
//!
//! There were none before this: everything the tool did was a one-shot
//! command with its own flags. The export directory is the first thing worth
//! remembering between runs, because it is where a checkout of the data set
//! lives on this machine and nobody wants to retype that path.
//!
//! `$XDG_CONFIG_HOME/gamebus-presenced/setup.toml`, beside the endpoint
//! overrides, and read with the same hand-written subset parser - a missing
//! file means defaults, an unreadable one means defaults, and no setting
//! here is ever a reason the tool cannot start.

use std::path::PathBuf;

pub(super) const CONFIG_NAME: &str = "setup.toml";

/// What the setup tool remembers between runs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SetupConfig {
    /// `[gamedb] export_dir`. `None` means the default below.
    pub(super) export_dir: Option<PathBuf>,
}

impl SetupConfig {
    /// Where `gamedb --export` writes without being told - the configured
    /// directory, else `<documents>/gamebus-gamedb`.
    pub(super) fn export_dir(&self) -> Option<PathBuf> {
        self.export_dir.clone().or_else(default_export_dir)
    }
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

pub(super) fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("gamebus-presenced").join(CONFIG_NAME))
}

/// Read the settings. Anything unreadable reads as "not configured".
pub(super) fn load() -> SetupConfig {
    let raw = config_path().and_then(|p| std::fs::read_to_string(p).ok());
    parse(raw.as_deref().unwrap_or_default())
}

/// Write the settings, atomically - the file is small, but a truncated one
/// would silently move the export somewhere the user never named.
pub(super) fn save(config: &SetupConfig) -> Result<(), String> {
    let path = config_path().ok_or("HOME is not set - nowhere to save the setting")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut text = String::from(
        "# Settings for gamebus-setup. Written by the tool; safe to edit by hand.\n\n",
    );
    text.push_str("[gamedb]\n");
    if let Some(dir) = &config.export_dir {
        text.push_str(
            "# Where 'gamebus-setup gamedb --export' writes pages, and what the TUI's\n\
             # gamedb tab uses. A checkout of gamebus-gamedb is the useful thing to\n\
             # point it at: the pages land in its games/ directory ready to commit.\n",
        );
        text.push_str(&format!(
            "export_dir = \"{}\"\n",
            dir.display()
                .to_string()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        ));
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot write {}: {e}", path.display())
        })
}

/// `[gamedb] export_dir` out of the file. Unknown sections and keys are
/// skipped rather than rejected: a newer tool's setting must not break an
/// older one.
fn parse(raw: &str) -> SetupConfig {
    let mut section = String::new();
    let mut config = SetupConfig::default();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        if section == "gamedb" && key.trim() == "export_dir" {
            config.export_dir = Some(PathBuf::from(expand_home(value)));
        }
    }
    config
}

/// The user's documents directory, as xdg-user-dirs records it.
///
/// `user-dirs.dirs` is a shell fragment, and the one line that matters looks
/// like `XDG_DOCUMENTS_DIR="$HOME/Dokumente"` - which is also why the
/// localized name has to be read rather than assumed. No file, or no such
/// line, falls back to `$HOME/Documents`.
pub(super) fn documents_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let from_file = config_dir()
        .map(|d| d.join("user-dirs.dirs"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| user_dir(&raw, "XDG_DOCUMENTS_DIR"))
        .map(|v| PathBuf::from(expand_home(&v)))
        .filter(|p| p.is_absolute());
    Some(from_file.unwrap_or_else(|| home.join("Documents")))
}

/// The default export directory: a sibling of the user's other documents,
/// named after the data set it holds.
pub(super) fn default_export_dir() -> Option<PathBuf> {
    Some(documents_dir()?.join("gamebus-gamedb"))
}

/// One `NAME="value"` assignment out of a `user-dirs.dirs` fragment.
fn user_dir(raw: &str, name: &str) -> Option<String> {
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// Expand a leading `$HOME` or `~`, which is how xdg-user-dirs writes every
/// path inside the home directory. Nothing else is expanded: this is a
/// configuration file, not a shell.
fn expand_home(value: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    for prefix in ["$HOME", "${HOME}", "~"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with('/') {
                return format!("{home}{rest}");
            }
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_or_broken_file_reads_as_no_setting() {
        assert_eq!(parse(""), SetupConfig::default());
        assert_eq!(
            parse("nonsense\n[gamedb]\nexport_dir = "),
            SetupConfig::default()
        );
        // A key outside the section, and an empty value, are both nothing.
        assert_eq!(parse("export_dir = \"/x\"\n"), SetupConfig::default());
        assert_eq!(
            parse("[gamedb]\nexport_dir = \"\"\n"),
            SetupConfig::default()
        );
        // An unknown section is skipped, not an error.
        assert_eq!(
            parse("[future]\nsomething = \"x\"\n[gamedb]\nexport_dir = \"/srv/gamedb\"\n")
                .export_dir
                .map(|p| p.display().to_string()),
            Some("/srv/gamedb".to_string())
        );
    }

    #[test]
    fn the_documents_line_is_read_out_of_the_xdg_fragment() {
        // The localized case is the reason the file is read at all.
        let raw = "# This file is written by xdg-user-dirs-update\n\
                   XDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"\n\
                   XDG_DOCUMENTS_DIR=\"$HOME/Dokumente\"\n\
                   XDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\n";
        assert_eq!(
            user_dir(raw, "XDG_DOCUMENTS_DIR").as_deref(),
            Some("$HOME/Dokumente")
        );
        assert_eq!(user_dir(raw, "XDG_MUSIC_DIR"), None);
        // An absolute path outside the home, and a commented-out line.
        assert_eq!(
            user_dir(
                "#XDG_DOCUMENTS_DIR=\"/nope\"\nXDG_DOCUMENTS_DIR=\"/srv/docs\"\n",
                "XDG_DOCUMENTS_DIR"
            )
            .as_deref(),
            Some("/srv/docs")
        );
        // A name that only starts the same must not match, and must not
        // stop the search either.
        assert_eq!(
            user_dir(
                "XDG_DOCUMENTS_DIRECTORY=\"/x\"\nXDG_DOCUMENTS_DIR=\"/srv/d\"\n",
                "XDG_DOCUMENTS_DIR"
            )
            .as_deref(),
            Some("/srv/d")
        );
    }

    #[test]
    fn only_a_leading_home_is_expanded() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(expand_home("$HOME/Dokumente"), format!("{home}/Dokumente"));
        assert_eq!(expand_home("${HOME}/Docs"), format!("{home}/Docs"));
        assert_eq!(expand_home("~/Docs"), format!("{home}/Docs"));
        assert_eq!(expand_home("/srv/docs"), "/srv/docs");
        // Not a path component boundary: left alone rather than mangled.
        assert_eq!(expand_home("$HOMEWORK/x"), "$HOMEWORK/x");
        assert_eq!(expand_home("~user/x"), "~user/x");
    }
}
