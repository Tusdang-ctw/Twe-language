//! Phase 33 session 3: integration tests for the stdlib JSON manifest.
//!
//! The manifest is the LLM grounding contract: every callable
//! appears exactly once, by canonical name, with the same params
//! `install()` registers. The tests below are drift catchers for
//! the install ↔ manifest pair.

use twec::stdlib::{manifest, manifest_to_json};

#[test]
fn manifest_size_meets_baseline() {
    // Phase 33 baseline; growth is fine, regression below this is a
    // signal that a category got dropped.
    let m = manifest();
    assert!(
        m.len() >= 200,
        "stdlib manifest only {} entries (baseline 200)",
        m.len()
    );
}

#[test]
fn every_category_has_at_least_one_builtin() {
    // The categories Tier 1 commits to expose. If any of these comes
    // up empty, the LLM's mental model loses a whole namespace.
    let m = manifest();
    let mut by_cat: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for spec in &m {
        *by_cat.entry(spec.category.as_str()).or_default() += 1;
    }
    // web3d-M0: `world` left this list when the Phase 32 bookkeeping
    // namespaces moved behind `--features experimental`.
    for cat in &[
        "math", "draw", "ui", "physics", "color", "random", "save", "net",
    ] {
        let n = by_cat.get(cat).copied().unwrap_or(0);
        assert!(n > 0, "category `{cat}` has zero entries");
    }
}

/// Namespaces that ship only with `--features experimental`
/// (docs/changes/2026-09-27-web3d-pivot.md).
const EXPERIMENTAL_NAMESPACES: &[&str] = &[
    "console.",
    "achievements.",
    "cloud_save.",
    "friends.",
    "mmo.",
    "workshop.",
    "rollback.",
    "world.",
    "terrain.",
];

#[cfg(not(feature = "experimental"))]
#[test]
fn default_manifest_excludes_experimental_namespaces() {
    // web3d-M0: the manifest is the LLM grounding contract. Scaffolding
    // namespaces (stub runtimes, bookkeeping the renderer never reads)
    // must not be advertised to models in the default build.
    for spec in &manifest() {
        assert!(
            !EXPERIMENTAL_NAMESPACES.iter().any(|ns| spec.name.starts_with(ns)),
            "`{}` is an experimental builtin but appears in the default manifest",
            spec.name
        );
    }
}

#[cfg(feature = "experimental")]
#[test]
fn experimental_manifest_includes_gated_namespaces() {
    let m = manifest();
    for ns in EXPERIMENTAL_NAMESPACES {
        assert!(
            m.iter().any(|s| s.name.starts_with(ns)),
            "`--features experimental` should install `{ns}*`"
        );
    }
}

#[test]
fn no_duplicate_canonical_names() {
    let m = manifest();
    let mut seen = std::collections::HashSet::new();
    for spec in &m {
        assert!(
            seen.insert(spec.name.clone()),
            "duplicate canonical name: `{}`",
            spec.name
        );
    }
}

#[test]
fn canonical_names_are_well_formed() {
    let m = manifest();
    for spec in &m {
        assert!(!spec.name.is_empty(), "empty name in manifest");
        // Each component is a valid identifier-shaped run; dots
        // separate namespace levels. No empty components, no
        // trailing dots.
        for part in spec.name.split('.') {
            assert!(
                !part.is_empty(),
                "empty component in `{}` (trailing or doubled dot?)",
                spec.name
            );
            assert!(
                part.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_'),
                "component `{part}` of `{}` doesn't start with a letter / underscore",
                spec.name
            );
        }
    }
}

#[test]
fn json_export_round_trips_count() {
    let m = manifest();
    let refs: Vec<&_> = m.iter().collect();
    let json = manifest_to_json(&refs);
    let needle = format!("\"count\":{}", m.len());
    assert!(
        json.contains(&needle),
        "JSON `count` field doesn't match manifest length"
    );
}

#[test]
fn json_export_is_valid_for_each_filter() {
    // A filtered view (e.g. `--category math`) must remain valid JSON
    // and report a count matching the filter result.
    let m = manifest();
    let math_only: Vec<&_> = m.iter().filter(|s| s.category == "math").collect();
    let json = manifest_to_json(&math_only);
    let needle = format!("\"count\":{}", math_only.len());
    assert!(json.contains(&needle));
    assert_eq!(json.matches('{').count(), json.matches('}').count());
    // Every spec in a math-filtered view must have category `math`.
    for spec in &math_only {
        assert_eq!(spec.category, "math");
    }
}
