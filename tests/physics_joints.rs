// web3d-M11 session 2: physics joints + CCD. A body tethered to a fixed
// anchor by a spherical joint hangs instead of falling; a free body
// falls. Physics is native-only (rapier); this binary isolates the
// process-wide thread-local world.

#![cfg(not(target_arch = "wasm32"))]

use twec::physics3d;

#[test]
fn spherical_joint_holds_a_body_up() {
    physics3d::reset();
    // A fixed anchor at y=5, a dynamic sphere just below it.
    let anchor = physics3d::static_sphere([0.0, 5.0, 0.0], 0.2);
    let link = physics3d::body("sphere", [0.0, 4.0, 0.0], 1.0).expect("body");
    // Tether link's centre to a point 1 unit below the anchor → it
    // should rest near y≈4 instead of falling.
    let j = physics3d::spherical_joint(anchor, link, [0.0, -1.0, 0.0], [0.0, 0.0, 0.0]);
    assert!(j.is_some(), "joint created");

    // A second, untethered body as the control.
    let free = physics3d::body("sphere", [3.0, 4.0, 0.0], 1.0).expect("free body");

    for _ in 0..90 {
        physics3d::step(1.0 / 60.0);
    }

    let link_y = physics3d::position(link).expect("link pos")[1];
    let free_y = physics3d::position(free).expect("free pos")[1];
    assert!(link_y > 3.0, "tethered body is held up (y={link_y})");
    assert!(free_y < 2.0, "free body fell (y={free_y})");

    // Removing the joint lets it fall.
    assert!(physics3d::remove_joint(j.unwrap()));
    for _ in 0..90 {
        physics3d::step(1.0 / 60.0);
    }
    assert!(
        physics3d::position(link).unwrap()[1] < link_y,
        "after removing the joint the body falls"
    );
    physics3d::reset();
}

#[test]
fn set_ccd_reports_known_and_unknown_bodies() {
    physics3d::reset();
    let b = physics3d::body("box", [0.0, 0.0, 0.0], 1.0).expect("body");
    assert!(physics3d::set_ccd(b, true), "CCD enabled on a real body");
    assert!(!physics3d::set_ccd(999_999, true), "unknown handle → false");
    // An unknown body handle for a joint is reported, not silently dropped.
    assert!(physics3d::spherical_joint(b, 999_999, [0.0; 3], [0.0; 3]).is_none());
    physics3d::reset();
}
