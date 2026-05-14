//! v1.0.1 session 11: CI perf-bench snapshot + regression alert.
//!
//! Keeps the Phase 8.5 3×-bytecode-VM speedup gap bounded by checking
//! a measured-vs-baseline ratio on every CI run. Pattern mirrors
//! Phase 35's `docs/api-snapshots/2026-05-10-baseline.json` —
//! deterministic JSON, FNV-1a hash for change-detection, exit-code
//! gating from the `twec perf-diff` subcommand.
//!
//! ## Workflow
//!
//! 1. Run `cargo bench` (Phase 11 session 6 / `benches/vm.rs`). Criterion
//!    writes `target/criterion/<group>/<id>/<base|new>/estimates.json`.
//! 2. Run `twec perf-snapshot -o current.json` to scrape the criterion
//!    output into the canonical JSON.
//! 3. Run `twec perf-diff <baseline.json> <current.json>` to fail CI
//!    when any reported bench regressed by more than the threshold
//!    (default 5%).
//!
//! ## Schema
//!
//! ```json
//! {
//!   "twec_version": "0.1.0",
//!   "captured_at_unix": 1747353600,
//!   "benches": {
//!     "sum_loop/backend/bytecode": { "median_ns": 12345.6 },
//!     "sum_loop/backend/tree":     { "median_ns": 23456.7 },
//!     "fib_recursive/backend/bytecode": { "median_ns": ... },
//!     ...
//!   }
//! }
//! ```
//!
//! Absolute numbers shift across machines — the *relative* ratio is
//! the load-bearing observable. Baselines live in
//! `docs/perf-snapshots/`; bench results from a new machine are
//! expected to differ in magnitude but should not regress past the
//! threshold on the same machine.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::json::{obj, parse as json_parse, to_string as json_to_string, Value};

/// One measured bench reported by criterion. Median nanoseconds is
/// the canonical statistic — robust to outlier samples and matches
/// the `target/criterion/.../estimates.json` `median.point_estimate`
/// field directly.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchEntry {
    pub median_ns: f64,
}

/// One serialised snapshot. `benches` is a sorted map so the on-disk
/// JSON is byte-stable across machines and Rust versions.
#[derive(Debug, Clone, PartialEq)]
pub struct PerfSnapshot {
    pub twec_version: String,
    pub captured_at_unix: u64,
    pub benches: BTreeMap<String, BenchEntry>,
}

impl PerfSnapshot {
    pub fn to_json(&self) -> String {
        let mut bench_obj: BTreeMap<String, Value> = BTreeMap::new();
        for (k, v) in &self.benches {
            bench_obj.insert(
                k.clone(),
                obj([("median_ns", Value::Float(v.median_ns))]),
            );
        }
        let body = obj([
            ("twec_version", Value::Str(self.twec_version.clone())),
            ("captured_at_unix", Value::Int(self.captured_at_unix as i64)),
            ("benches", Value::Object(bench_obj)),
        ]);
        json_to_string(&body)
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let v = json_parse(text).map_err(|e| format!("perf snapshot: bad JSON: {e}"))?;
        let twec_version = v
            .get("twec_version")
            .and_then(|x| x.as_str())
            .ok_or("perf snapshot: missing `twec_version`")?
            .to_string();
        let captured_at_unix = v
            .get("captured_at_unix")
            .and_then(|x| x.as_i64())
            .ok_or("perf snapshot: missing `captured_at_unix`")? as u64;
        let benches_v = v.get("benches").ok_or("perf snapshot: missing `benches`")?;
        let benches_map = match benches_v {
            Value::Object(m) => m,
            _ => return Err("perf snapshot: `benches` is not an object".into()),
        };
        let mut benches = BTreeMap::new();
        for (k, val) in benches_map {
            let median_ns = val
                .get("median_ns")
                .and_then(|x| match x {
                    Value::Float(f) => Some(*f),
                    Value::Int(i) => Some(*i as f64),
                    _ => None,
                })
                .ok_or_else(|| format!("perf snapshot: bench `{k}` missing `median_ns`"))?;
            benches.insert(k.clone(), BenchEntry { median_ns });
        }
        Ok(Self {
            twec_version,
            captured_at_unix,
            benches,
        })
    }
}

/// Scrape `target/criterion/` for every bench group / id pair and
/// return a `PerfSnapshot` keyed by `"<group>/<id-pair>"`. Criterion
/// writes one estimates.json per bench under
/// `target/criterion/<group>/<id1>/<id2>/new/estimates.json`. Some
/// projects have a single `new/` directly under the bench leaf; we
/// walk down to the deepest `new/estimates.json` we can find at
/// each leaf.
pub fn scrape_criterion(target_dir: &Path) -> Result<PerfSnapshot, String> {
    let criterion = target_dir.join("criterion");
    if !criterion.exists() {
        return Err(format!(
            "perf snapshot: no `{}` directory — did you run `cargo bench`?",
            criterion.display()
        ));
    }
    let mut benches: BTreeMap<String, BenchEntry> = BTreeMap::new();
    // The `report/` directory is criterion's HTML output index — skip
    // it. Every other top-level entry is a bench group.
    for entry in fs::read_dir(&criterion).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if name == "report" {
            continue;
        }
        collect_estimates(&path, name, &mut benches)?;
    }
    Ok(PerfSnapshot {
        twec_version: env!("CARGO_PKG_VERSION").to_string(),
        captured_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        benches,
    })
}

/// Walk `dir` looking for `new/estimates.json` or `base/estimates.json`.
/// `prefix` is the slash-joined criterion path so far (used as the
/// key the bench is reported under). Subdirectories named `report`
/// are skipped (criterion's HTML output).
fn collect_estimates(
    dir: &Path,
    prefix: &str,
    out: &mut BTreeMap<String, BenchEntry>,
) -> Result<(), String> {
    // Prefer `new/` over `base/` — `new/` is the latest run. If
    // both exist (criterion writes `new/` then rotates to `base/`),
    // `new/` is the freshly captured one.
    let new = dir.join("new").join("estimates.json");
    let base = dir.join("base").join("estimates.json");
    let leaf = if new.exists() {
        Some(new)
    } else if base.exists() {
        Some(base)
    } else {
        None
    };
    if let Some(p) = leaf {
        let body = fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
        let median_ns = parse_estimates_median(&body)
            .ok_or_else(|| format!("{}: missing median.point_estimate", p.display()))?;
        out.insert(prefix.to_string(), BenchEntry { median_ns });
        return Ok(());
    }
    // Not a leaf — recurse into named subdirs (excluding criterion's
    // bookkeeping directories).
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if matches!(name, "report" | "new" | "base" | "change") {
            continue;
        }
        let next_prefix = format!("{prefix}/{name}");
        collect_estimates(&path, &next_prefix, out)?;
    }
    Ok(())
}

/// Pull `median.point_estimate` out of criterion's estimates.json
/// body. We hand-parse rather than depend on serde because Twe's
/// build avoids casual deps.
fn parse_estimates_median(body: &str) -> Option<f64> {
    let v = json_parse(body).ok()?;
    let median = v.get("median")?;
    let point = median.get("point_estimate")?;
    match point {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    }
}

/// One row of `twec perf-diff` output. `pct` is the *signed* delta
/// — positive means current is slower than baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRow {
    pub name: String,
    pub baseline_ns: f64,
    pub current_ns: f64,
    pub pct: f64,
}

/// Compare two snapshots. Returns one [`DiffRow`] per bench that
/// appears in both snapshots. Benches only in baseline or only in
/// current are reported separately (callers print them; they don't
/// trip the regression gate because there's nothing to compare).
pub fn diff(baseline: &PerfSnapshot, current: &PerfSnapshot) -> DiffResult {
    let mut rows: Vec<DiffRow> = Vec::new();
    let mut only_baseline: Vec<String> = Vec::new();
    let mut only_current: Vec<String> = Vec::new();
    for (k, b) in &baseline.benches {
        match current.benches.get(k) {
            Some(c) => {
                let pct = if b.median_ns > 0.0 {
                    (c.median_ns - b.median_ns) / b.median_ns * 100.0
                } else {
                    0.0
                };
                rows.push(DiffRow {
                    name: k.clone(),
                    baseline_ns: b.median_ns,
                    current_ns: c.median_ns,
                    pct,
                });
            }
            None => only_baseline.push(k.clone()),
        }
    }
    for k in current.benches.keys() {
        if !baseline.benches.contains_key(k) {
            only_current.push(k.clone());
        }
    }
    DiffResult {
        rows,
        only_baseline,
        only_current,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffResult {
    pub rows: Vec<DiffRow>,
    pub only_baseline: Vec<String>,
    pub only_current: Vec<String>,
}

impl DiffResult {
    /// True when any row's pct exceeds the threshold. The threshold
    /// is interpreted as a *positive* percentage (5.0 → 5%); negative
    /// rows (current faster than baseline) are always fine.
    pub fn regressed(&self, threshold_pct: f64) -> bool {
        self.rows.iter().any(|r| r.pct > threshold_pct)
    }

    /// Human-friendly report. The CLI prints this; CI logs it on
    /// failure so a contributor can see exactly which bench tripped
    /// the gate.
    pub fn format_human(&self, threshold_pct: f64) -> String {
        let mut out = String::new();
        if self.rows.is_empty() {
            out.push_str("perf-diff: no bench appears in both snapshots\n");
        } else {
            out.push_str("perf-diff (threshold = +");
            out.push_str(&format!("{threshold_pct:.1}%"));
            out.push_str("):\n");
            // Sort by descending pct so regressions surface first.
            let mut sorted = self.rows.clone();
            sorted.sort_by(|a, b| b.pct.partial_cmp(&a.pct).unwrap_or(std::cmp::Ordering::Equal));
            for r in &sorted {
                let tag = if r.pct > threshold_pct {
                    "REGRESS"
                } else if r.pct < -threshold_pct {
                    "faster "
                } else {
                    "ok     "
                };
                out.push_str(&format!(
                    "  [{tag}] {name:<40}  {pct:+6.2}%   (baseline {bl:.1}ns → current {cur:.1}ns)\n",
                    name = r.name,
                    pct = r.pct,
                    bl = r.baseline_ns,
                    cur = r.current_ns,
                ));
            }
        }
        for k in &self.only_baseline {
            out.push_str(&format!("  [missing] `{k}` not in current snapshot\n"));
        }
        for k in &self.only_current {
            out.push_str(&format!("  [new]     `{k}` not in baseline snapshot\n"));
        }
        out
    }
}

/// Read a snapshot from disk. Convenience for the CLI entry points.
pub fn read_snapshot(path: &Path) -> Result<PerfSnapshot, String> {
    let body = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    PerfSnapshot::from_json(&body)
}

/// Write a snapshot to disk. Pretty-printing is left to the JSON
/// module's default (compact); the schema is hashable for the
/// stability-audit pattern Phase 35 introduced.
pub fn write_snapshot(snap: &PerfSnapshot, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
    }
    fs::write(path, snap.to_json()).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Default regression threshold matching the v1.0.1 plan: >5% is a
/// failure, ≤5% is noise tolerance.
pub const DEFAULT_THRESHOLD_PCT: f64 = 5.0;

#[allow(dead_code)]
pub fn default_target_dir() -> PathBuf {
    PathBuf::from("target")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(entries: &[(&str, f64)]) -> PerfSnapshot {
        let mut benches = BTreeMap::new();
        for (k, v) in entries {
            benches.insert(
                (*k).to_string(),
                BenchEntry { median_ns: *v },
            );
        }
        PerfSnapshot {
            twec_version: "test".into(),
            captured_at_unix: 0,
            benches,
        }
    }

    #[test]
    fn diff_flags_regression_above_threshold() {
        let baseline = snap(&[("sum_loop/bytecode", 100.0)]);
        let current = snap(&[("sum_loop/bytecode", 110.0)]); // +10%
        let d = diff(&baseline, &current);
        assert!(d.regressed(5.0), "10% regression should trip 5% threshold");
        assert!(!d.regressed(15.0), "10% regression should NOT trip 15% threshold");
    }

    #[test]
    fn diff_ignores_improvements() {
        let baseline = snap(&[("sum_loop/bytecode", 100.0)]);
        let current = snap(&[("sum_loop/bytecode", 80.0)]); // -20% (faster)
        let d = diff(&baseline, &current);
        assert!(!d.regressed(5.0), "improvements should never trip the gate");
        assert_eq!(d.rows.len(), 1);
        assert!(d.rows[0].pct < 0.0);
    }

    #[test]
    fn snapshot_json_round_trips() {
        let original = snap(&[
            ("a/bytecode", 100.5),
            ("b/tree", 200.0),
        ]);
        let body = original.to_json();
        let parsed = PerfSnapshot::from_json(&body).expect("parse");
        assert_eq!(parsed.benches.len(), 2);
        assert_eq!(parsed.benches["a/bytecode"].median_ns, 100.5);
        assert_eq!(parsed.benches["b/tree"].median_ns, 200.0);
    }

    #[test]
    fn diff_reports_only_in_baseline_or_current() {
        let baseline = snap(&[
            ("a", 100.0),
            ("shared", 200.0),
        ]);
        let current = snap(&[
            ("shared", 210.0),
            ("c", 50.0),
        ]);
        let d = diff(&baseline, &current);
        assert_eq!(d.only_baseline, vec!["a".to_string()]);
        assert_eq!(d.only_current, vec!["c".to_string()]);
        assert_eq!(d.rows.len(), 1);
        assert_eq!(d.rows[0].name, "shared");
    }

    #[test]
    fn format_human_marks_regress_and_faster() {
        let baseline = snap(&[
            ("regressed", 100.0),
            ("improved", 100.0),
            ("noise", 100.0),
        ]);
        let current = snap(&[
            ("regressed", 200.0), // +100%
            ("improved", 50.0),   // -50%
            ("noise", 101.0),     // +1% — under threshold
        ]);
        let d = diff(&baseline, &current);
        let s = d.format_human(5.0);
        assert!(s.contains("REGRESS"), "missing REGRESS tag: {s}");
        assert!(s.contains("faster"), "missing faster tag: {s}");
        assert!(s.contains("ok"), "missing ok tag: {s}");
    }
}
