// web3d-M9 session 1: `twec asset info` inspector tests. These run
// against committed .glb fixtures (no GPU, no network) and assert the
// structural facts + VRAM estimate + JSON shape.

use std::path::Path;
use twec::asset_info::{inspect, ImageFormat, VRAM_BUDGET_BYTES};
use twec::json;

const HERO: &str = "examples/survive3d/assets/hero.glb";
const TRIANGLE: &str = "examples/assets/triangle.glb";

#[test]
fn inspects_skinned_hero() {
    let report = inspect(Path::new(HERO)).expect("hero inspects");
    let g = report.gltf.as_ref().expect("hero is gltf");
    assert_eq!(g.meshes, 1);
    assert_eq!(g.primitives, 1);
    assert_eq!(g.total_vertices, 288);
    assert_eq!(g.total_indices, 432);
    // The hero is skinned + animated with one texture.
    assert_eq!(g.skins, 1);
    assert!(g.attrs.joints && g.attrs.weights && g.attrs.normals && g.attrs.uv0);
    assert_eq!(g.animations, 2);
    assert_eq!(g.images.len(), 1);
    assert_eq!(g.images[0].format, ImageFormat::Png);
    // Single-skin, no morph, no compression ⇒ no M9 gaps flagged.
    assert!(report.gaps.is_empty(), "unexpected gaps: {:?}", report.gaps);
    assert!(!report.vram.over_budget());
}

#[test]
fn inspects_minimal_triangle() {
    let report = inspect(Path::new(TRIANGLE)).expect("triangle inspects");
    let g = report.gltf.as_ref().expect("triangle is gltf");
    assert_eq!(g.total_vertices, 3);
    assert_eq!(g.skins, 0);
    assert_eq!(g.animations, 0);
    assert_eq!(g.images.len(), 0);
    assert_eq!(report.vram.texture_bytes, 0);
}

#[test]
fn vram_estimate_within_budget_for_fixtures() {
    for p in [HERO, TRIANGLE] {
        let report = inspect(Path::new(p)).expect("inspects");
        assert!(report.vram.total_bytes < VRAM_BUDGET_BYTES);
        assert_eq!(report.vram.budget_bytes, VRAM_BUDGET_BYTES);
        assert_eq!(
            report.vram.total_bytes,
            report.vram.geometry_bytes + report.vram.texture_bytes
        );
    }
}

#[test]
fn json_output_is_valid_and_structured() {
    let report = inspect(Path::new(HERO)).expect("inspects");
    let text = report.to_json();
    let v = json::parse(&text).expect("asset info JSON parses");
    assert_eq!(v.get("kind").and_then(|k| k.as_str()), Some("gltf"));
    let g = v.get("gltf").expect("gltf object");
    assert_eq!(g.get("animations").and_then(|a| a.as_i64()), Some(2));
    assert_eq!(g.get("skins").and_then(|s| s.as_i64()), Some(1));
    let vram = v.get("vram").expect("vram object");
    assert!(vram.get("budget_bytes").and_then(|b| b.as_i64()).is_some());
}

#[test]
fn compression_extensions_detects_draco_without_strict_parse() {
    // The strict gltf loader rejects a Draco file; the raw reader still
    // reports the compression extension (used by the build audit).
    let found = twec::asset_info::compression_extensions(Path::new(
        "examples/assets/compressed/Box_draco.gltf",
    ));
    assert_eq!(found, vec!["KHR_draco_mesh_compression"]);
    // A plain file reports nothing.
    assert!(twec::asset_info::compression_extensions(Path::new(TRIANGLE)).is_empty());
}

#[test]
fn rejects_non_asset_file() {
    // The project's own Cargo.toml is not a glTF or an image.
    let err = inspect(Path::new("Cargo.toml"));
    assert!(err.is_err());
}
