//! Rendering the systemd unit and the D-Bus activation file.
//!
//! Both files name the daemon by absolute path, so neither can be installed
//! verbatim outside `/usr`. The systemd unit could have used `%h`, but the
//! D-Bus activation file's `Exec=` takes **no** specifiers at all — no `%h`,
//! no `$HOME`, no expansion of any kind. A user-level install therefore has to
//! generate both.
//!
//! The templates below are the same text as `data/*.service`, which stay in the
//! tree as the artifacts a distribution package installs directly. The golden
//! test at the bottom renders with `/usr/bin` and asserts byte equality with
//! those files, so the two representations cannot drift apart.

use std::path::Path;

const SYSTEMD_UNIT: &str = "\
[Unit]
Description=gamebus-presenced - D-Bus presence daemon for Linux desktop
Documentation=https://github.com/fschaupp/gamebus-presenced

[Service]
Type=dbus
BusName=org.gamebus.Presence.v1
ExecStart={exec}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
";

const DBUS_SERVICE: &str = "\
[D-BUS Service]
Name=org.gamebus.Presence.v1
Exec={exec}
SystemdService=gamebus-presenced.service
";

/// The systemd **user** unit, with the daemon's absolute path substituted.
pub fn render_systemd_unit(exec: &Path) -> String {
    SYSTEMD_UNIT.replace("{exec}", &exec.display().to_string())
}

/// The D-Bus session-bus activation file, with the same path.
pub fn render_dbus_service(exec: &Path) -> String {
    DBUS_SERVICE.replace("{exec}", &exec.display().to_string())
}

/// The `ExecStart=`/`Exec=` value out of an installed unit or activation file.
///
/// Used to catch the most common broken install: files left pointing at a
/// binary that has moved or was never there.
pub fn parse_exec(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("ExecStart=")
            .or_else(|| line.strip_prefix("Exec="))
            .map(|v| v.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Pins the rendered output to the files a distribution package ships.
    /// If someone edits `data/*.service` without touching the templates (or the
    /// reverse), this fails.
    #[test]
    fn rendering_with_usr_bin_reproduces_the_shipped_data_files() {
        let exec = PathBuf::from("/usr/bin/gamebus-presenced");
        assert_eq!(
            render_systemd_unit(&exec),
            include_str!("../../data/gamebus-presenced.service")
        );
        assert_eq!(
            render_dbus_service(&exec),
            include_str!("../../data/org.gamebus.Presence.v1.service")
        );
    }

    #[test]
    fn user_install_gets_the_expanded_path_in_both_files() {
        let exec = PathBuf::from("/home/tester/.local/bin/gamebus-presenced");
        let unit = render_systemd_unit(&exec);
        let service = render_dbus_service(&exec);

        assert!(unit.contains("ExecStart=/home/tester/.local/bin/gamebus-presenced\n"));
        assert!(service.contains("Exec=/home/tester/.local/bin/gamebus-presenced\n"));
        // No specifier or shell variable survives into either file: the D-Bus
        // activation file would not expand them.
        for text in [&unit, &service] {
            assert!(!text.contains("%h"), "specifier left in: {text}");
            assert!(!text.contains('$'), "shell variable left in: {text}");
        }
    }

    #[test]
    fn exec_parses_back_out_of_both_rendered_files() {
        let exec = PathBuf::from("/home/tester/.local/bin/gamebus-presenced");
        assert_eq!(
            parse_exec(&render_systemd_unit(&exec)).as_deref(),
            Some("/home/tester/.local/bin/gamebus-presenced")
        );
        assert_eq!(
            parse_exec(&render_dbus_service(&exec)).as_deref(),
            Some("/home/tester/.local/bin/gamebus-presenced")
        );
        assert_eq!(parse_exec("[Unit]\nDescription=nothing here\n"), None);
    }
}
