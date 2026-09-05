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

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::Path;

/// What the MangoHud header line carries. Every field is optional: the
/// `driver` column is routinely empty, and a newer MangoHud may drop or
/// reorder columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Specs {
    pub os: Option<String>,
    pub cpu: Option<String>,
    /// The GPU device string - the one that actually ran this session, which
    /// on a multi-GPU machine is the only correct answer and is why this is
    /// read per trace rather than probed from the running system.
    pub gpu: Option<String>,
    /// RAM as MangoHud reports it, in KiB.
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
        assert_eq!(s.gpu.as_deref(), Some("Intel(R) Arc(tm) A770 Graphics (DG2)"));
        assert_eq!(s.ram_kib, Some(32210576));
        assert_eq!(s.kernel.as_deref(), Some("7.3.0-0.1-fls-upstream-upstream+"));
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
        assert_eq!(s.gpu.as_deref(), Some("Intel(R) Arc(tm) A770 Graphics (DG2)"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
