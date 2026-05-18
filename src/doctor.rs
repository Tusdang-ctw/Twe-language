//! v1.0.1 session 13: `twec doctor` — diagnostic command.
//!
//! Saves 80% of "why doesn't `play3d` work on my machine" issues
//! during the v1.0 community ramp by collecting everything a Twe
//! maintainer would ask for in the first triage round and printing
//! it in one place. Optional `--json` output keeps it machine-
//! readable for the LLM-grounded support workflow per
//! [`crate::doctor`]'s P4 rationale in `docs/v1.0.1-plan.md` §13.
//!
//! Intentionally **not** invasive: no wgpu probe (would spin up an
//! adapter purely for the report; slow + may itself fail and confuse
//! the very users this command exists to help), no network calls,
//! no spawning of game windows. Everything the report shows is
//! readable from the filesystem or compile-time `cfg!`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Snapshot of everything `twec doctor` reports. Pure data — the
/// formatter consumes this and decides text-vs-JSON output. Tests
/// build a `Report` directly without touching the filesystem and
/// assert against `to_json`.
#[derive(Debug)]
pub struct Report {
    pub twec_version: String,
    pub os: String,
    pub arch: String,
    pub family: String,
    pub features: Features,
    pub crash_dir: PathBuf,
    pub recent_crashes: Vec<CrashEntry>,
    pub cache_dir: Option<PathBuf>,
    pub cache_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Features {
    pub steam: bool,
    pub steam_net: bool,
}

#[derive(Debug)]
pub struct CrashEntry {
    pub path: PathBuf,
    /// Unix seconds parsed from the `twec-crash-<secs>-<pid>.log`
    /// filename. Falls back to the file's mtime if the parse fails.
    pub secs_since_epoch: u64,
    pub bytes: u64,
}

impl Report {
    /// Build a `Report` by inspecting the current environment.
    pub fn capture() -> Self {
        let crash_dir = effective_crash_dir();
        let (recent_crashes, scan_warning) = scan_crash_dir(&crash_dir);
        let mut warnings: Vec<String> = Vec::new();
        if let Some(w) = scan_warning {
            warnings.push(w);
        }
        let cache_dir = effective_cache_dir();
        let cache_bytes = match cache_dir.as_ref() {
            Some(d) => match dir_size(d) {
                Ok(b) => b,
                Err(e) => {
                    warnings.push(format!(
                        "could not measure cache_dir {}: {}",
                        d.display(),
                        e
                    ));
                    0
                }
            },
            None => 0,
        };
        Report {
            twec_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            family: std::env::consts::FAMILY.to_string(),
            features: detect_features(),
            crash_dir,
            recent_crashes,
            cache_dir,
            cache_bytes,
            warnings,
        }
    }

    /// Render the report as a UTF-8 JSON document. Hand-rolled to
    /// match the no-serde convention in the rest of the crate.
    pub fn to_json(&self) -> String {
        let mut s = String::with_capacity(512);
        s.push('{');
        s.push_str("\"tool\":\"twec-doctor\",\"version\":1,");
        push_field(&mut s, "twec_version", &self.twec_version);
        s.push(',');
        push_field(&mut s, "os", &self.os);
        s.push(',');
        push_field(&mut s, "arch", &self.arch);
        s.push(',');
        push_field(&mut s, "family", &self.family);
        s.push_str(",\"features\":{");
        s.push_str(&format!("\"steam\":{}", self.features.steam));
        s.push_str(&format!(",\"steam_net\":{}", self.features.steam_net));
        s.push('}');
        s.push_str(",\"crash_dir\":");
        push_str_value(&mut s, &self.crash_dir.display().to_string());
        s.push_str(",\"recent_crashes\":[");
        for (i, c) in self.recent_crashes.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('{');
            s.push_str("\"path\":");
            push_str_value(&mut s, &c.path.display().to_string());
            s.push_str(",\"secs_since_epoch\":");
            s.push_str(&c.secs_since_epoch.to_string());
            s.push_str(",\"bytes\":");
            s.push_str(&c.bytes.to_string());
            s.push('}');
        }
        s.push(']');
        s.push_str(",\"cache_dir\":");
        match &self.cache_dir {
            Some(p) => push_str_value(&mut s, &p.display().to_string()),
            None => s.push_str("null"),
        }
        s.push_str(",\"cache_bytes\":");
        s.push_str(&self.cache_bytes.to_string());
        s.push_str(",\"warnings\":[");
        for (i, w) in self.warnings.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            push_str_value(&mut s, w);
        }
        s.push(']');
        s.push('}');
        s
    }

    /// Render the report as human-readable text. Reads like a
    /// support-ticket attachment.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str("twec doctor\n");
        s.push_str("===========\n");
        s.push_str(&format!("twec version : {}\n", self.twec_version));
        s.push_str(&format!(
            "platform     : {} ({}, {})\n",
            self.os, self.arch, self.family
        ));
        s.push_str("features     :");
        let mut any = false;
        if self.features.steam {
            s.push_str(" steam");
            any = true;
        }
        if self.features.steam_net {
            s.push_str(" steam-net");
            any = true;
        }
        if !any {
            s.push_str(" (none enabled)");
        }
        s.push('\n');
        s.push_str(&format!("crash dir    : {}\n", self.crash_dir.display()));
        s.push_str("recent crashes:\n");
        if self.recent_crashes.is_empty() {
            s.push_str("  (none)\n");
        } else {
            for c in &self.recent_crashes {
                s.push_str(&format!(
                    "  {} ({} bytes, t={}s)\n",
                    c.path.display(),
                    c.bytes,
                    c.secs_since_epoch
                ));
            }
        }
        match &self.cache_dir {
            Some(p) => s.push_str(&format!(
                "cache dir    : {} ({} bytes)\n",
                p.display(),
                self.cache_bytes
            )),
            None => s.push_str("cache dir    : (unset; set TWEC_CACHE_DIR to track)\n"),
        }
        if !self.warnings.is_empty() {
            s.push_str("warnings:\n");
            for w in &self.warnings {
                s.push_str(&format!("  - {}\n", w));
            }
        }
        s
    }
}

fn detect_features() -> Features {
    Features {
        steam: cfg!(feature = "steam"),
        steam_net: cfg!(feature = "steam-net"),
    }
}

fn effective_crash_dir() -> PathBuf {
    std::env::var_os("TWEC_CRASH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn effective_cache_dir() -> Option<PathBuf> {
    std::env::var_os("TWEC_CACHE_DIR").map(PathBuf::from)
}

/// Scan `dir` for `twec-crash-<secs>-<pid>.log` filenames; return
/// the three most-recent (largest `<secs>`) entries. Errors are
/// rolled into a single warning string so the report always has
/// *some* output even if the dir is unreadable.
fn scan_crash_dir(dir: &Path) -> (Vec<CrashEntry>, Option<String>) {
    let mut entries: Vec<CrashEntry> = Vec::new();
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            return (
                Vec::new(),
                Some(format!("could not read crash_dir {}: {}", dir.display(), e)),
            );
        }
    };
    for entry in read.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.starts_with("twec-crash-") || !name.ends_with(".log") {
            continue;
        }
        let secs = parse_crash_secs(&name).unwrap_or_else(|| mtime_secs(&path));
        let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        entries.push(CrashEntry {
            path,
            secs_since_epoch: secs,
            bytes,
        });
    }
    entries.sort_by(|a, b| b.secs_since_epoch.cmp(&a.secs_since_epoch));
    entries.truncate(3);
    (entries, None)
}

fn parse_crash_secs(name: &str) -> Option<u64> {
    // `twec-crash-<secs>-<pid>.log`
    let stripped = name.strip_prefix("twec-crash-")?;
    let stripped = stripped.strip_suffix(".log")?;
    let dash = stripped.find('-')?;
    stripped[..dash].parse::<u64>().ok()
}

fn mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_secs()
        })
        .unwrap_or(0)
}

fn dir_size(dir: &Path) -> io::Result<u64> {
    let mut total = 0u64;
    if !dir.exists() {
        return Ok(0);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let md = entry.metadata()?;
        if md.is_dir() {
            total = total.saturating_add(dir_size(&entry.path())?);
        } else {
            total = total.saturating_add(md.len());
        }
    }
    Ok(total)
}

fn push_field(s: &mut String, k: &str, v: &str) {
    s.push('"');
    s.push_str(k);
    s.push_str("\":");
    push_str_value(s, v);
}

fn push_str_value(s: &mut String, v: &str) {
    s.push('"');
    for c in v.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
}

/// Determines current time — exposed so callers can stamp the
/// `captured_at_unix` field of any wrapper they build.
pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;

    /// Capture in a tempdir-redirected env so we know what we put in
    /// the crash dir and can assert against the report. Uses a single
    /// per-test dir under `std::env::temp_dir()` keyed by test name.
    fn tempdir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("twec-doctor-test-{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir tempdir");
        dir
    }

    #[test]
    fn parse_crash_secs_extracts_unix_seconds_from_name() {
        assert_eq!(
            parse_crash_secs("twec-crash-1747353600-12345.log"),
            Some(1747353600)
        );
        // Missing pid section.
        assert_eq!(parse_crash_secs("twec-crash-1747353600.log"), None);
        // Wrong prefix.
        assert_eq!(parse_crash_secs("crash-1747353600-12345.log"), None);
        // Wrong suffix.
        assert_eq!(parse_crash_secs("twec-crash-1747353600-12345.txt"), None);
    }

    #[test]
    fn scan_crash_dir_picks_three_most_recent() {
        let dir = tempdir("scan-three");
        // Write five crash files with monotonically increasing secs.
        for secs in [100u64, 200, 300, 400, 500] {
            let p = dir.join(format!("twec-crash-{secs}-1.log"));
            fs::write(&p, b"x").unwrap();
        }
        // Plus an unrelated file that should be ignored.
        fs::write(dir.join("twec-crash-irrelevant.txt"), b"x").unwrap();
        let (entries, warn) = scan_crash_dir(&dir);
        assert!(warn.is_none());
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].secs_since_epoch, 500);
        assert_eq!(entries[1].secs_since_epoch, 400);
        assert_eq!(entries[2].secs_since_epoch, 300);
    }

    #[test]
    fn scan_crash_dir_warns_on_unreadable_dir() {
        let (entries, warn) = scan_crash_dir(Path::new("/does/not/exist/twec-test"));
        assert!(entries.is_empty());
        assert!(warn.is_some(), "expected warning, got {:?}", warn);
    }

    #[test]
    fn dir_size_sums_recursively() {
        let dir = tempdir("dir-size");
        fs::write(dir.join("a"), b"hello").unwrap(); // 5
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/b"), b"worlds").unwrap(); // 6
        assert_eq!(dir_size(&dir).unwrap(), 11);
    }

    #[test]
    fn report_json_round_trips_known_shape() {
        let report = Report {
            twec_version: "0.1.0".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            family: "unix".to_string(),
            features: Features {
                steam: false,
                steam_net: false,
            },
            crash_dir: PathBuf::from("/tmp/cd"),
            recent_crashes: vec![CrashEntry {
                path: PathBuf::from("/tmp/cd/twec-crash-100-1.log"),
                secs_since_epoch: 100,
                bytes: 42,
            }],
            cache_dir: None,
            cache_bytes: 0,
            warnings: vec!["w1".to_string()],
        };
        let body = report.to_json();
        // Spot-check the load-bearing fields are present + correctly
        // typed (numbers unquoted, strings quoted).
        assert!(body.contains("\"tool\":\"twec-doctor\""));
        assert!(body.contains("\"twec_version\":\"0.1.0\""));
        assert!(body.contains("\"os\":\"linux\""));
        assert!(body.contains("\"secs_since_epoch\":100"));
        assert!(body.contains("\"bytes\":42"));
        assert!(body.contains("\"cache_dir\":null"));
        assert!(body.contains("\"warnings\":[\"w1\"]"));
        assert!(body.contains("\"features\":{\"steam\":false,\"steam_net\":false}"));
    }

    #[test]
    fn report_text_lists_no_crashes_when_empty() {
        let report = Report {
            twec_version: "0.1.0".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            family: "unix".to_string(),
            features: Features::default(),
            crash_dir: PathBuf::from("/tmp"),
            recent_crashes: Vec::new(),
            cache_dir: None,
            cache_bytes: 0,
            warnings: Vec::new(),
        };
        let text = report.to_text();
        assert!(text.contains("recent crashes:\n  (none)"));
        assert!(text.contains("cache dir    : (unset"));
    }

    #[test]
    fn capture_returns_populated_basics() {
        // Smoke-test the live capture path — non-test data must at
        // least include a version + os + family. Validates only
        // shape, not specific values, because the host varies.
        let r = Report::capture();
        assert!(!r.twec_version.is_empty());
        assert!(!r.os.is_empty());
        assert!(!r.family.is_empty());
    }
}
