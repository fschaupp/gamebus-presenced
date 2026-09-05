//! Machine specs for a report, read from the trace itself.
//!
//! A ProtonDB report wants CPU, GPU, RAM, kernel and distro. beisl does not
//! relay them and should not: gamebus-setup runs on the same machine, so a
//! relay would add surface without adding information, and a full spec tuple
//! is a decent machine fingerprint that has no business in beisl's exportable
//! artifact or on its MCP surface (that side's reasoning, 2026-09-05, and it
//! is right). Consent to publish specs belongs at the submit step, in front of
//! the user.
//!
//! So this side reads them, and reads them LATE. The scan stores only a
//! pointer to the run directory; nothing copies a fingerprint into the stash
//! ahead of a decision to publish one.
//!
//! The source is the MangoHud CSV beisl leaves in a run directory. Its first
//! two lines are a spec header and its values; the frame data starts on line
//! three and runs to hundreds of thousands of lines, so this reads exactly two
//! lines and stops.
//!
//! Best effort by design: the CSV only exists when MangoHud logged the
//! session. On this machine that was 3 of 12 runs, so "no specs" is an
//! ordinary outcome, not an error.
//!
//! # Why the trace usually has none, for exactly our findings
//!
//! A game stopped by a compat wall never renders a frame, so MangoHud never
//! writes a CSV (Florian, 2026-09-05, on the WARDOGS runs). Missing trace
//! specs therefore correlate almost perfectly with the findings most worth
//! filing - the correlation is structural, not luck. [`read_system`] is the
//! answer to that: the machine can be asked directly, at the same late
//! moment and with the same rule that nothing is stored.
//!
//! The two sources are NOT interchangeable and [`SpecSource`] keeps them
//! apart. A trace names the GPU that actually ran the session; a probe can
//! only list the GPUs present, and this machine has two. Reporting "the GPU"
//! from a probe on a multi-GPU box would be a confident wrong answer.

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::Path;

/// Where a set of specs came from, and therefore what may be claimed of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpecSource {
    /// The run's own MangoHud CSV: scoped to that session, so `gpu` is the
    /// device that actually ran the game.
    #[default]
    Trace,
    /// Probed from this machine now. Says nothing about which GPU ran a
    /// game, or even whether the machine is the one that did.
    System,
}

/// What the MangoHud header line carries. Every field is optional: the
/// `driver` column is routinely empty, and a newer MangoHud may drop or
/// reorder columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Specs {
    pub source: SpecSource,
    pub os: Option<String>,
    pub cpu: Option<String>,
    /// The GPU device string - the one that actually ran this session, which
    /// on a multi-GPU machine is the only correct answer and is why this is
    /// read per trace rather than probed from the running system.
    pub gpu: Option<String>,
    /// Every GPU present, filled by [`read_system`] only. None of them is
    /// known to have run any particular game - that is the whole difference
    /// between this and `gpu`.
    pub gpus: Vec<String>,
    /// RAM in KiB.
    pub ram_kib: Option<u64>,
    pub kernel: Option<String>,
    pub driver: Option<String>,
    pub cpu_scheduler: Option<String>,
}

impl Specs {
    /// Whether anything at all was readable.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn ram_gib(&self) -> Option<f64> {
        self.ram_kib.map(|k| k as f64 / 1024.0 / 1024.0)
    }

    /// A one-line rendering for review. Only the parts that are known.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(cpu) = &self.cpu {
            parts.push(cpu.clone());
        }
        if let Some(gpu) = &self.gpu {
            parts.push(gpu.clone());
        }
        if !self.gpus.is_empty() {
            parts.push(self.gpus.join(" + "));
        }
        if let Some(gib) = self.ram_gib() {
            parts.push(format!("{gib:.0} GiB"));
        }
        if let Some(kernel) = &self.kernel {
            parts.push(kernel.clone());
        }
        parts.join(" / ")
    }

    /// Parse a MangoHud header pair.
    ///
    /// Column-name driven rather than positional, so a MangoHud that adds or
    /// reorders columns still yields what it does carry. A row whose width
    /// does not match the header is refused outright: misaligned fields would
    /// put a kernel version in a CPU field, and a wrong spec in a public
    /// report is worse than no spec.
    pub fn parse(header: &str, values: &str) -> Option<Self> {
        let names: Vec<&str> = header.trim().split(',').map(str::trim).collect();
        let vals: Vec<&str> = values.trim().split(',').map(str::trim).collect();
        if names.len() != vals.len() || names.first() != Some(&"os") {
            return None;
        }
        let mut specs = Self::default();
        for (name, value) in names.iter().zip(vals) {
            let value = (!value.is_empty()).then(|| value.to_string());
            match *name {
                "os" => specs.os = value,
                "cpu" => specs.cpu = value,
                "gpu" => specs.gpu = value,
                "ram" => specs.ram_kib = value.and_then(|v| v.parse().ok()),
                "kernel" => specs.kernel = value,
                "driver" => specs.driver = value,
                "cpuscheduler" => specs.cpu_scheduler = value,
                _ => {}
            }
        }
        (!specs.is_empty()).then_some(specs)
    }
}

/// Read the specs out of a run directory, if a MangoHud CSV is there.
///
/// Reads two lines and stops - the rest of the file is frame data and can run
/// to hundreds of thousands of lines.
pub fn read_from_dir(dir: &Path) -> Option<Specs> {
    let csv = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv")))?;
    let file = std::fs::File::open(csv).ok()?;
    let mut lines = BufReader::new(file).lines();
    let header = lines.next()?.ok()?;
    let values = lines.next()?.ok()?;
    Specs::parse(&header, &values)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "os,cpu,gpu,ram,kernel,driver,cpuscheduler";
    const VALUES: &str = "Steam Runtime 4,AMD Ryzen 7 7700X 8-Core Processor,\
Intel(R) Arc(tm) A770 Graphics (DG2),32210576,7.3.0-0.1-fls-upstream-upstream+,,performance";

    #[test]
    fn a_real_mangohud_header_parses() {
        let s = Specs::parse(HEADER, VALUES).expect("parsed");
        assert_eq!(s.cpu.as_deref(), Some("AMD Ryzen 7 7700X 8-Core Processor"));
        assert_eq!(
            s.gpu.as_deref(),
            Some("Intel(R) Arc(tm) A770 Graphics (DG2)")
        );
        assert_eq!(s.ram_kib, Some(32210576));
        assert_eq!(
            s.kernel.as_deref(),
            Some("7.3.0-0.1-fls-upstream-upstream+")
        );
        assert_eq!(s.os.as_deref(), Some("Steam Runtime 4"));
    }

    #[test]
    fn an_empty_column_is_absent_not_empty_string() {
        let s = Specs::parse(HEADER, VALUES).unwrap();
        assert_eq!(s.driver, None, "the driver column is routinely empty");
    }

    #[test]
    fn ram_converts_to_gib_for_a_report() {
        let s = Specs::parse(HEADER, VALUES).unwrap();
        assert_eq!(format!("{:.0}", s.ram_gib().unwrap()), "31");
    }

    #[test]
    fn a_misaligned_row_is_refused_rather_than_shifted() {
        // One value short: positional parsing would slide kernel into driver.
        let short = "Steam Runtime 4,AMD Ryzen 7,Intel Arc,32210576,7.3.0,";
        assert!(Specs::parse(HEADER, short).is_none());
    }

    #[test]
    fn a_frame_data_header_is_not_mistaken_for_specs() {
        let frames = "fps,frametime,cpu_load,gpu_load";
        assert!(Specs::parse(frames, "60,16.6,10,90").is_none());
    }

    #[test]
    fn a_reordered_or_extended_header_still_yields_what_it_carries() {
        let h = "os,gpu,cpu,ram,kernel,driver,cpuscheduler,newcolumn";
        let v = "Arch,Radeon RX 7900 XT,Ryzen 9,16777216,6.11.0,mesa,performance,whatever";
        let s = Specs::parse(h, v).expect("parsed");
        assert_eq!(s.gpu.as_deref(), Some("Radeon RX 7900 XT"));
        assert_eq!(s.cpu.as_deref(), Some("Ryzen 9"));
        assert_eq!(s.driver.as_deref(), Some("mesa"));
    }

    #[test]
    fn the_summary_names_only_what_is_known() {
        let s = Specs {
            gpu: Some("Intel Arc A770".into()),
            ..Default::default()
        };
        assert_eq!(s.summary(), "Intel Arc A770");
    }

    #[test]
    fn a_directory_with_no_csv_yields_nothing() {
        let dir = std::env::temp_dir().join(format!("specs-none-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(read_from_dir(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_first_two_lines_are_read() {
        let dir = std::env::temp_dir().join(format!("specs-big-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut body = format!("{HEADER}\n{VALUES}\nfps,frametime\n");
        // The real files run to hundreds of thousands of frame rows.
        body.push_str(&"60,16.6\n".repeat(50_000));
        std::fs::write(dir.join("game_2026-08-22.csv"), body).unwrap();
        let s = read_from_dir(&dir).expect("specs read");
        assert_eq!(
            s.gpu.as_deref(),
            Some("Intel(R) Arc(tm) A770 Graphics (DG2)")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Ask this machine for what a report needs, when the trace could not say.
///
/// Every source is a plain file this process can already read - no
/// subprocess, no privilege. Read at review and submit time like the trace
/// path, and stored nowhere for the same reason.
///
/// `gpu` is deliberately left empty: a probe can enumerate the GPUs present
/// but cannot know which one ran a game, so they go in `gpus` and the caller
/// is obliged to say so.
pub fn read_system() -> Specs {
    Specs {
        source: SpecSource::System,
        os: os_pretty_name(),
        cpu: first_field("/proc/cpuinfo", "model name"),
        gpu: None,
        gpus: present_gpus(),
        ram_kib: first_field("/proc/meminfo", "MemTotal")
            .and_then(|v| v.split_whitespace().next().and_then(|n| n.parse().ok())),
        kernel: read_trimmed("/proc/sys/kernel/osrelease"),
        driver: None,
        cpu_scheduler: None,
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// First `key: value` line in a `/proc` file.
fn first_field(path: &str, key: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.trim() == key).then(|| value.trim().to_string())
        })
}

fn os_pretty_name() -> Option<String> {
    let raw = std::fs::read_to_string("/etc/os-release").ok()?;
    raw.lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
}

/// Every render node's device, named through the system's PCI id table when
/// there is one. Falls back to `driver 1002:73df`, which is still more use in
/// a bug report than nothing.
fn present_gpus() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let ids = PciIds::load();
    let mut gpus = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `card0` is a device; `card0-DP-5` is a connector on it.
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let Ok(uevent) = std::fs::read_to_string(entry.path().join("device/uevent")) else {
            continue;
        };
        let field = |key: &str| {
            uevent
                .lines()
                .find_map(|l| l.strip_prefix(key))
                .map(str::to_ascii_lowercase)
        };
        let Some(pci) = field("PCI_ID=") else {
            continue;
        };
        let driver = field("DRIVER=").unwrap_or_default();
        let Some((vendor, device)) = pci.split_once(':') else {
            continue;
        };
        match ids.as_ref().and_then(|i| i.name(vendor, device)) {
            Some(name) => gpus.push(name),
            None if !driver.is_empty() => gpus.push(format!("{driver} {vendor}:{device}")),
            None => gpus.push(format!("{vendor}:{device}")),
        }
    }
    gpus.sort();
    gpus
}

/// The system's `pci.ids` table, held as text and searched on demand - it is
/// well over a megabyte and this looks up one or two devices.
struct PciIds(String);

impl PciIds {
    fn load() -> Option<Self> {
        ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
            .into_iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
            .map(Self)
    }

    /// Device name for a lowercase `vendor`/`device` id pair.
    ///
    /// The file lists vendors unindented and their devices one tab in, so a
    /// device is only this vendor's while no new unindented line has been
    /// passed. Without that guard a device id would match under the wrong
    /// vendor, which is how a Radeon ends up labelled as somebody else's part.
    fn name(&self, vendor: &str, device: &str) -> Option<String> {
        let mut in_vendor = false;
        for line in self.0.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if !line.starts_with('\t') {
                if in_vendor {
                    return None; // left our vendor without a hit
                }
                in_vendor = line
                    .split_once("  ")
                    .is_some_and(|(id, _)| id.eq_ignore_ascii_case(vendor));
                continue;
            }
            if !in_vendor || line.starts_with("\t\t") {
                continue;
            }
            let entry = line.trim_start_matches('\t');
            if let Some((id, name)) = entry.split_once("  ") {
                if id.eq_ignore_ascii_case(device) {
                    return Some(name.trim().to_string());
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod system_tests {
    use super::*;

    const IDS: &str = "\
# comment
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t73df  Navi 22 [Radeon RX 6700/6700 XT/6750 XT]
\t\t1043 0000  Some subsystem
8086  Intel Corporation
\t56a0  DG2 [Arc A770]
";

    #[test]
    fn a_device_resolves_under_its_own_vendor() {
        let ids = PciIds(IDS.into());
        assert_eq!(
            ids.name("1002", "73df").as_deref(),
            Some("Navi 22 [Radeon RX 6700/6700 XT/6750 XT]")
        );
        assert_eq!(ids.name("8086", "56a0").as_deref(), Some("DG2 [Arc A770]"));
    }

    #[test]
    fn a_device_never_matches_under_the_wrong_vendor() {
        let ids = PciIds(IDS.into());
        // 56a0 exists, but not under AMD.
        assert_eq!(ids.name("1002", "56a0"), None);
        assert_eq!(ids.name("8086", "73df"), None);
    }

    #[test]
    fn a_subsystem_line_is_not_mistaken_for_a_device() {
        let ids = PciIds(IDS.into());
        assert_eq!(
            ids.name("1002", "1043"),
            None,
            "two tabs deep is a subsystem"
        );
    }

    #[test]
    fn an_unknown_id_names_nothing() {
        assert_eq!(PciIds(IDS.into()).name("dead", "beef"), None);
    }

    #[test]
    fn the_probe_reads_this_machine() {
        // Runs on the CI/dev box it is compiled on: /proc is always there.
        let s = read_system();
        assert_eq!(s.source, SpecSource::System);
        assert!(s.kernel.is_some(), "kernel comes from /proc");
        assert!(s.cpu.is_some(), "cpu comes from /proc/cpuinfo");
        assert!(s.ram_kib.is_some_and(|k| k > 0), "ram comes from /proc");
        assert!(
            s.gpu.is_none(),
            "a probe must never claim which GPU ran a game"
        );
    }
}
