//! Where everything goes, for each install target. Pure path arithmetic.

use std::path::PathBuf;

pub const DAEMON_BIN: &str = "gamebus-presenced";
pub const CLI_BIN: &str = "gamebus-presence";
pub const SETUP_BIN: &str = "gamebus-setup";
pub const UNIT_NAME: &str = "gamebus-presenced.service";
pub const DBUS_SERVICE_NAME: &str = "org.gamebus.Presence.v1.service";
pub const DETECTABLE_NAME: &str = "detectable.json";

/// Where an install writes to.
///
/// `System` means the system *prefix*, not the system bus - the unit stays a
/// systemd **user** unit, one daemon per login session. It only changes where
/// the files live and who may write them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    User,
    System,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::User => "user",
            Target::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Target::User),
            "system" => Some(Target::System),
            _ => None,
        }
    }
}

/// The user's XDG base directories, resolved once at the edge of the program.
///
/// Everything downstream takes a `&Dirs` rather than reading the environment,
/// so the path logic stays pure and testable.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub cache_home: PathBuf,
    pub runtime_dir: PathBuf,
    /// System data directories in XDG order, for mirroring the daemon's
    /// naming-database search. Not used by `layout` - see its note on why the
    /// system layout must stay free of the environment.
    pub data_dirs: Vec<PathBuf>,
}

impl Dirs {
    /// Resolve from the environment. `None` when `HOME` is unset - every
    /// user-level path depends on it and guessing would write to the wrong place.
    pub fn from_env() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let xdg = |var: &str, default: &str| -> PathBuf {
            std::env::var_os(var)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(default))
        };
        Some(Self {
            config_home: xdg("XDG_CONFIG_HOME", ".config"),
            data_home: xdg("XDG_DATA_HOME", ".local/share"),
            cache_home: xdg("XDG_CACHE_HOME", ".cache"),
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir),
            data_dirs: xdg_data_dirs(),
            home,
        })
    }
}

/// System data directories, per the XDG base directory specification.
///
/// The same resolution as `xdg_data_dirs()` in `src/naming.rs`; duplicated
/// because the daemon's modules are not compiled into this binary. The default
/// is `/usr/local/share:/usr/share`.
fn xdg_data_dirs() -> Vec<PathBuf> {
    std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        })
}

/// The four directories an install touches, for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub target: Target,
    pub bin_dir: PathBuf,
    pub data_dir: PathBuf,
    pub unit_dir: PathBuf,
    pub dbus_service_dir: PathBuf,
}

impl Layout {
    pub fn daemon_bin(&self) -> PathBuf {
        self.bin_dir.join(DAEMON_BIN)
    }
    pub fn cli_bin(&self) -> PathBuf {
        self.bin_dir.join(CLI_BIN)
    }
    pub fn setup_bin(&self) -> PathBuf {
        self.bin_dir.join(SETUP_BIN)
    }
    pub fn unit_file(&self) -> PathBuf {
        self.unit_dir.join(UNIT_NAME)
    }
    pub fn dbus_service_file(&self) -> PathBuf {
        self.dbus_service_dir.join(DBUS_SERVICE_NAME)
    }
    pub fn detectable_file(&self) -> PathBuf {
        self.data_dir.join(DETECTABLE_NAME)
    }

    /// The installed reference copy of the endpoint configuration. User
    /// overrides belong in the config dir, not here - install rewrites this
    /// one with the bundled content.
    pub fn endpoints_file(&self) -> PathBuf {
        self.data_dir.join(crate::endpoints::ENDPOINTS_NAME)
    }

    /// The installed reference copy of the shared-helper list. User additions
    /// belong in the config dir, not here - the lists union, so this copy can
    /// only add entries; install rewrites it with the bundled content.
    pub fn shared_helpers_file(&self) -> PathBuf {
        self.data_dir.join(crate::naming::SHARED_HELPERS_NAME)
    }
    /// The three binaries, in install order.
    pub fn binaries(&self) -> [(&'static str, PathBuf); 3] {
        [
            (DAEMON_BIN, self.daemon_bin()),
            (CLI_BIN, self.cli_bin()),
            (SETUP_BIN, self.setup_bin()),
        ]
    }
}

/// Resolve the layout for a target.
///
/// The `System` arm reads **nothing** from `dirs` - that is load-bearing.
/// A system install re-executes this binary under `pkexec`, which scrubs the
/// environment, so the privileged process must arrive at the same paths as the
/// unprivileged one that planned them. See the test at the bottom of this file.
///
/// The system prefix is `/usr/local`, the FHS home for locally built software,
/// and it is not configurable. `/usr` is what a distribution package owns:
/// installing there from a hand-run tool collides with any future package at
/// the same paths with nothing tracking ownership, and fails outright on the
/// image-based distributions much of Linux gaming runs on. All three
/// `/usr/local` locations are searched by default - it is in `XDG_DATA_DIRS`,
/// and `/usr/local/lib/systemd/user` is in systemd's user unit path.
pub fn layout(dirs: &Dirs, target: Target) -> Layout {
    match target {
        Target::User => Layout {
            target,
            // Not an XDG variable - ~/.local/bin is the systemd-file-hierarchy
            // convention, and what distributions put on PATH by default.
            bin_dir: dirs.home.join(".local/bin"),
            data_dir: dirs.data_home.join("gamebus-presenced"),
            unit_dir: dirs.config_home.join("systemd/user"),
            dbus_service_dir: dirs.data_home.join("dbus-1/services"),
        },
        Target::System => Layout {
            target,
            bin_dir: PathBuf::from("/usr/local/bin"),
            data_dir: PathBuf::from("/usr/local/share/gamebus-presenced"),
            unit_dir: PathBuf::from("/usr/local/lib/systemd/user"),
            dbus_service_dir: PathBuf::from("/usr/local/share/dbus-1/services"),
        },
    }
}

/// Which of the naming database's search locations a file was found in.
///
/// **Mirrors `find_detectable_json()` in `src/naming.rs` - keep the two in the
/// same order.** If they drift, this tool reports a tier the daemon does not use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectableTier {
    /// `$XDG_CACHE_HOME/gamebus-presenced/` - written by `fetch-detectable`.
    Cache,
    /// `$XDG_DATA_HOME/gamebus-presenced/` - a user-level install.
    UserData,
    /// A system data directory from `XDG_DATA_DIRS` - a system install.
    SystemData,
}

impl DetectableTier {
    pub fn as_str(self) -> &'static str {
        match self {
            DetectableTier::Cache => "cache",
            DetectableTier::UserData => "user data",
            DetectableTier::SystemData => "system data",
        }
    }
}

/// The naming database search path, highest priority first.
pub fn detectable_candidates(dirs: &Dirs) -> Vec<(DetectableTier, PathBuf)> {
    let mut candidates = vec![
        (
            DetectableTier::Cache,
            dirs.cache_home
                .join("gamebus-presenced")
                .join(DETECTABLE_NAME),
        ),
        (
            DetectableTier::UserData,
            dirs.data_home
                .join("gamebus-presenced")
                .join(DETECTABLE_NAME),
        ),
    ];
    candidates.extend(dirs.data_dirs.iter().map(|d| {
        (
            DetectableTier::SystemData,
            d.join("gamebus-presenced").join(DETECTABLE_NAME),
        )
    }));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(home: &str) -> Dirs {
        let home = PathBuf::from(home);
        Dirs {
            config_home: home.join(".config"),
            data_home: home.join(".local/share"),
            cache_home: home.join(".cache"),
            runtime_dir: PathBuf::from("/run/user/1000"),
            data_dirs: vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ],
            home,
        }
    }

    #[test]
    fn user_layout_follows_the_home_directory() {
        let l = layout(&dirs("/home/tester"), Target::User);
        assert_eq!(
            l.daemon_bin(),
            PathBuf::from("/home/tester/.local/bin/gamebus-presenced")
        );
        assert_eq!(
            l.unit_file(),
            PathBuf::from("/home/tester/.config/systemd/user/gamebus-presenced.service")
        );
        assert_eq!(
            l.dbus_service_file(),
            PathBuf::from(
                "/home/tester/.local/share/dbus-1/services/org.gamebus.Presence.v1.service"
            )
        );
        assert_eq!(
            l.detectable_file(),
            PathBuf::from("/home/tester/.local/share/gamebus-presenced/detectable.json")
        );
    }

    /// A system install is re-executed under `pkexec`, which resets the
    /// environment. If any system path depended on `Dirs`, the privileged
    /// process would write somewhere other than what the user confirmed.
    #[test]
    fn system_layout_ignores_the_environment_entirely() {
        let sane = layout(&dirs("/home/tester"), Target::System);
        let garbage = layout(&dirs("/nonsense/../etc"), Target::System);
        assert_eq!(sane, garbage);
        assert_eq!(
            sane.daemon_bin(),
            PathBuf::from("/usr/local/bin/gamebus-presenced")
        );
        assert_eq!(
            sane.unit_file(),
            PathBuf::from("/usr/local/lib/systemd/user/gamebus-presenced.service")
        );
        assert_eq!(
            sane.detectable_file(),
            PathBuf::from("/usr/local/share/gamebus-presenced/detectable.json")
        );
        // /usr is what a distribution package owns; a hand-run install must
        // not write there.
        for path in [sane.daemon_bin(), sane.unit_file(), sane.detectable_file()] {
            assert!(
                path.starts_with("/usr/local"),
                "system install escaped /usr/local: {}",
                path.display()
            );
        }
    }

    #[test]
    fn detectable_search_order_matches_the_daemon() {
        let c = detectable_candidates(&dirs("/home/tester"));
        let tiers: Vec<_> = c.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            tiers,
            vec![
                DetectableTier::Cache,
                DetectableTier::UserData,
                // One per XDG_DATA_DIRS entry, in order.
                DetectableTier::SystemData,
                DetectableTier::SystemData,
            ]
        );
        assert_eq!(
            c[2].1,
            PathBuf::from("/usr/local/share/gamebus-presenced/detectable.json")
        );
        assert_eq!(
            c[0].1,
            PathBuf::from("/home/tester/.cache/gamebus-presenced/detectable.json")
        );
    }

    #[test]
    fn target_round_trips_through_its_string() {
        for t in [Target::User, Target::System] {
            assert_eq!(Target::parse(t.as_str()), Some(t));
        }
        assert_eq!(Target::parse("both"), None);
    }
}
