# Perf snapshots

v1.0.1 session 11 — checked-in baseline + format reference for the
`twec perf-snapshot` / `twec perf-diff` regression gate.

## Files

- `v1.0.1-baseline.json` — the starter baseline. **Numbers are
  placeholder approximations** captured from Phase 11 closeout notes,
  not from a real bench run on shared infrastructure. The first CI
  run that produces a clean bench should recapture and commit this
  file. The bench-set shape (group/id keys) is canonical; the absolute
  median_ns values aren't load-bearing yet.

## Workflow

```bash
# 1. Run the criterion bench suite.
cargo bench --bench vm

# 2. Scrape criterion's output into a snapshot JSON.
twec perf-snapshot -o current.json

# 3. Compare against the checked-in baseline.
twec perf-diff docs/perf-snapshots/v1.0.1-baseline.json current.json
```

The diff is human-readable on stdout; exit code 0 means clean, 1
means at least one bench regressed beyond the threshold (default
5%, override with `--threshold N`).

## Recapture a baseline

A maintainer who confirms current numbers are an acceptable new
baseline:

```bash
cargo bench --bench vm
twec perf-snapshot -o docs/perf-snapshots/v1.0.1-baseline.json
git add docs/perf-snapshots/v1.0.1-baseline.json
git commit -m "perf: recapture v1.0.1 baseline"
```

The schema is a deterministic JSON object so the diff in PR review
is greppable. New benches added to `benches/vm.rs` automatically
appear in subsequent snapshots — the gate only fires on benches
present in *both* baseline and current, so an additive bench
doesn't break CI.
