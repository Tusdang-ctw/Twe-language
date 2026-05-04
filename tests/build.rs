// Phase 12 sessions 1+: `twec build` orchestration tests.
//
// We stand up tiny in-tree project trees inside the `target/` test
// staging area and exercise discover / validate / dry-run end-to-end.
// Tests that need real artifacts (sessions 4+) live alongside; this
// file holds the non-IO-heavy ones.

use std::fs;
use std::path::PathBuf;
use twec::build::{discover_project, validate_project, BuildConfig, BuildTarget};

fn temp_project(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("twec_build_{}_{}", name, std::process::id()));
    if base.exists() {
        let _ = fs::remove_dir_all(&base);
    }
    fs::create_dir_all(&base).expect("create temp project");
    base
}

#[test]
fn discover_finds_main_and_walks_assets() {
    let dir = temp_project("discover_basic");
    fs::write(dir.join("main.twe"), "print(\"hi\")\n").unwrap();
    fs::create_dir_all(dir.join("assets/sprites")).unwrap();
    fs::write(dir.join("assets/walk.png"), b"\x89PNG fake").unwrap();
    fs::write(dir.join("assets/sprites/hero.png"), b"\x89PNG fake").unwrap();
    fs::write(dir.join("assets/audio.ogg"), b"OggS fake").unwrap();

    let project = discover_project(&dir).expect("discover");
    assert_eq!(project.name, dir.file_name().unwrap().to_string_lossy());
    assert_eq!(project.assets.len(), 3);
    let keys: Vec<_> = project.assets.iter().map(|a| a.bundle_key.as_str()).collect();
    // Sorted, forward-slash-canonical bundle keys.
    assert_eq!(
        keys,
        vec!["assets/audio.ogg", "assets/sprites/hero.png", "assets/walk.png"]
    );
    assert!(project.manifest.is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_picks_up_optional_manifest() {
    let dir = temp_project("discover_manifest");
    fs::write(dir.join("main.twe"), "print(\"hi\")\n").unwrap();
    fs::write(dir.join("twe.toml"), "[project]\nname=\"x\"\n").unwrap();
    let project = discover_project(&dir).expect("discover");
    assert!(project.manifest.is_some());
    assert!(project.assets.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_errors_when_main_missing() {
    let dir = temp_project("discover_missing");
    let err = discover_project(&dir).expect_err("should fail without main.twe");
    assert!(err.contains("main.twe"), "got: {err}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discover_errors_when_path_not_a_directory() {
    let dir = temp_project("discover_not_a_dir");
    let file = dir.join("main.twe");
    fs::write(&file, "print(\"hi\")\n").unwrap();
    let err = discover_project(&file).expect_err("should fail when given a file");
    assert!(err.contains("not a directory") || err.contains("missing"), "got: {err}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn validate_succeeds_on_well_formed_program() {
    let dir = temp_project("validate_ok");
    fs::write(
        dir.join("main.twe"),
        "let x = 1\nlet y = x + 2\nprint(y)\n",
    )
    .unwrap();
    let project = discover_project(&dir).expect("discover");
    validate_project(&project).expect("well-formed program should validate");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn validate_reports_parse_error_with_path() {
    let dir = temp_project("validate_parse_err");
    // Unterminated string is a hard lex / parse failure.
    fs::write(dir.join("main.twe"), "let x = \"oops\nlet y = x\n").unwrap();
    let project = discover_project(&dir).expect("discover");
    let err = validate_project(&project).expect_err("should fail");
    assert!(
        err.contains("main.twe"),
        "error should reference main.twe: {err}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn target_host_returns_a_real_target() {
    // `BuildTarget::host` must always produce a valid target so the
    // CLI default works on any contributor's machine.
    let host = BuildTarget::host();
    let label = host.label();
    assert!(BuildTarget::parse(label).is_some(), "host label round-trips");
}

#[test]
fn config_round_trips() {
    for c in [BuildConfig::Dev, BuildConfig::Release, BuildConfig::Profile] {
        assert_eq!(BuildConfig::parse(c.label()), Some(c));
    }
}
