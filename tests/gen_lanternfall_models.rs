//! Lanternfall G0: every voxel model of the game, built in code.
//!
//! Writes each `examples/lanternfall/assets/*.glb` if it is missing, then
//! checks it is the file its recipe builds and that the engine loads it.
//! The files are committed, so players never run this; it is the
//! procedural spec of the art (edit a recipe, delete the file, rerun to
//! regenerate), like `gen_action3d_hero.rs`, which this generalises:
//! a model is box parts coloured from a palette texture, optionally
//! skinned to a joint tree with keyframed clips.
//!
//! Every character faces +Z (what `look: facing: 0` faces), feet at
//! y = 0, about 1.6 m tall: a big hooded head, a small body, and a
//! lantern on its back — the island's "lanternfolk".

use std::fmt::Write as _;
use std::path::Path;

const CELL: usize = 16;

/// A box part: (min, max, joint, palette cell), bind-pose metres.
type Part = ([f32; 3], [f32; 3], u16, usize);

/// One animated property of one joint.
enum Track {
    /// Rotation about `axis` (unit), degrees per key.
    Rotate {
        joint: usize,
        axis: [f32; 3],
        deg: &'static [f32],
    },
    /// Translation offset from the joint's rest position, per key.
    Offset {
        joint: usize,
        by: &'static [[f32; 3]],
    },
}

struct Clip {
    name: &'static str,
    times: &'static [f32],
    tracks: Vec<Track>,
}

struct Model {
    name: &'static str,
    palette: Vec<[u8; 3]>,
    /// (name, parent, translation relative to the parent). Empty: an
    /// unskinned prop.
    joints: Vec<(&'static str, Option<usize>, [f32; 3])>,
    parts: Vec<Part>,
    clips: Vec<Clip>,
    /// Lanternfall G4: instead of box parts, an open tube of this many
    /// segments — radius 1, y from 0 to 1, seen from both sides (the
    /// safe zone's wall).
    tube: Option<usize>,
}

// ---------------------------------------------------------------
// The models.
// ---------------------------------------------------------------

/// Character palette cells.
const SKIN: usize = 0;
const SHIRT: usize = 1;
const PANTS: usize = 2;
const DARK: usize = 3;
const HOOD: usize = 4;
const PACK: usize = 5;
const LAMP: usize = 6;

const X: [f32; 3] = [1.0, 0.0, 0.0];
const Z: [f32; 3] = [0.0, 0.0, 1.0];

/// Joints of every character: hips at the feet (so a fall pivots on the
/// ground), legs, arms, head.
fn character_joints() -> Vec<(&'static str, Option<usize>, [f32; 3])> {
    vec![
        ("hips", None, [0.0, 0.0, 0.0]),
        ("leg_l", Some(0), [-0.13, 0.45, 0.0]),
        ("leg_r", Some(0), [0.13, 0.45, 0.0]),
        ("arm_l", Some(0), [-0.33, 1.0, 0.0]),
        ("arm_r", Some(0), [0.33, 1.0, 0.0]),
        ("head", Some(0), [0.0, 1.05, 0.0]),
    ]
}

fn character_parts() -> Vec<Part> {
    vec![
        // body
        ([-0.25, 0.45, -0.16], [0.25, 1.05, 0.16], 0, SHIRT),
        // backpack with its lantern
        ([-0.18, 0.55, -0.3], [0.18, 0.95, -0.16], 0, PACK),
        ([-0.07, 0.95, -0.27], [0.07, 1.12, -0.17], 0, LAMP),
        // head: face, hood, eyes
        ([-0.26, 1.05, -0.24], [0.26, 1.52, 0.26], 5, SKIN),
        ([-0.29, 1.46, -0.3], [0.29, 1.62, 0.29], 5, HOOD),
        ([-0.29, 1.05, -0.3], [0.29, 1.47, -0.22], 5, HOOD),
        ([-0.29, 1.12, -0.22], [-0.25, 1.47, 0.18], 5, HOOD),
        ([0.25, 1.12, -0.22], [0.29, 1.47, 0.18], 5, HOOD),
        ([-0.17, 1.26, 0.26], [-0.06, 1.35, 0.28], 5, DARK),
        ([0.06, 1.26, 0.26], [0.17, 1.35, 0.28], 5, DARK),
        // legs and boots
        ([-0.21, 0.09, -0.09], [-0.05, 0.46, 0.09], 1, PANTS),
        ([-0.22, 0.0, -0.1], [-0.04, 0.11, 0.13], 1, DARK),
        ([0.05, 0.09, -0.09], [0.21, 0.46, 0.09], 2, PANTS),
        ([0.04, 0.0, -0.1], [0.22, 0.11, 0.13], 2, DARK),
        // arms and hands
        ([-0.41, 0.6, -0.08], [-0.25, 1.03, 0.08], 3, SHIRT),
        ([-0.41, 0.5, -0.08], [-0.25, 0.6, 0.08], 3, SKIN),
        ([0.25, 0.6, -0.08], [0.41, 1.03, 0.08], 4, SHIRT),
        ([0.25, 0.5, -0.08], [0.41, 0.6, 0.08], 4, SKIN),
    ]
}

const IDLE_T: &[f32] = &[0.0, 1.0, 2.0];
const RUN_T: &[f32] = &[0.0, 0.15, 0.3, 0.45, 0.6];
const HOLD_T: &[f32] = &[0.0, 1.0];

fn character_clips() -> Vec<Clip> {
    // Arms raised forward to hold a weapon (rotation about X: -80° points
    // a hanging arm along +Z).
    let armed = |times: &'static [f32], l: &'static [f32], r: &'static [f32]| {
        vec![
            Track::Rotate {
                joint: 3,
                axis: X,
                deg: l,
            },
            Track::Rotate {
                joint: 4,
                axis: X,
                deg: r,
            },
        ]
        .into_iter()
        .chain(std::iter::once(Track::Rotate {
            joint: 0,
            axis: X,
            deg: &[0.0; 5][..times.len()],
        }))
        .collect::<Vec<_>>()
    };
    let run_legs = || {
        vec![
            Track::Rotate {
                joint: 1,
                axis: X,
                deg: &[0.0, 50.0, 0.0, -50.0, 0.0],
            },
            Track::Rotate {
                joint: 2,
                axis: X,
                deg: &[0.0, -50.0, 0.0, 50.0, 0.0],
            },
            Track::Offset {
                joint: 0,
                by: &[
                    [0.0, 0.07, 0.0],
                    [0.0, 0.0, 0.0],
                    [0.0, 0.07, 0.0],
                    [0.0, 0.0, 0.0],
                    [0.0, 0.07, 0.0],
                ],
            },
        ]
    };
    let mut run_armed = run_legs();
    run_armed.extend(armed(
        RUN_T,
        &[-70.0, -74.0, -70.0, -74.0, -70.0],
        &[-82.0, -86.0, -82.0, -86.0, -82.0],
    ));
    let mut idle_armed = vec![Track::Offset {
        joint: 0,
        by: &[[0.0, 0.0, 0.0], [0.0, 0.015, 0.0], [0.0, 0.0, 0.0]],
    }];
    idle_armed.extend(armed(
        IDLE_T,
        &[-68.0, -70.0, -68.0],
        &[-80.0, -82.0, -80.0],
    ));
    vec![
        Clip {
            name: "idle",
            times: IDLE_T,
            tracks: vec![
                Track::Offset {
                    joint: 0,
                    by: &[[0.0, 0.0, 0.0], [0.0, 0.015, 0.0], [0.0, 0.0, 0.0]],
                },
                Track::Rotate {
                    joint: 3,
                    axis: X,
                    deg: &[0.0, 4.0, 0.0],
                },
                Track::Rotate {
                    joint: 4,
                    axis: X,
                    deg: &[0.0, -4.0, 0.0],
                },
                Track::Rotate {
                    joint: 5,
                    axis: X,
                    deg: &[0.0, 2.0, 0.0],
                },
            ],
        },
        Clip {
            name: "run",
            times: RUN_T,
            tracks: {
                let mut t = run_legs();
                t.push(Track::Rotate {
                    joint: 3,
                    axis: X,
                    deg: &[0.0, -42.0, 0.0, 42.0, 0.0],
                });
                t.push(Track::Rotate {
                    joint: 4,
                    axis: X,
                    deg: &[0.0, 42.0, 0.0, -42.0, 0.0],
                });
                t
            },
        },
        Clip {
            name: "idle_armed",
            times: IDLE_T,
            tracks: idle_armed,
        },
        Clip {
            name: "run_armed",
            times: RUN_T,
            tracks: run_armed,
        },
        // A one-shot swing: the right arm winds up overhead and comes down.
        Clip {
            name: "melee",
            times: &[0.0, 0.12, 0.32],
            tracks: vec![
                Track::Rotate {
                    joint: 4,
                    axis: X,
                    deg: &[0.0, -150.0, -20.0],
                },
                Track::Rotate {
                    joint: 3,
                    axis: X,
                    deg: &[0.0, 20.0, -10.0],
                },
                Track::Rotate {
                    joint: 1,
                    axis: X,
                    deg: &[0.0, -15.0, 0.0],
                },
            ],
        },
        // Fall backwards onto the ground, arms flung up.
        Clip {
            name: "death",
            times: &[0.0, 0.18, 0.45, 0.6],
            tracks: vec![
                Track::Rotate {
                    joint: 0,
                    axis: X,
                    deg: &[0.0, -25.0, -88.0, -84.0],
                },
                Track::Rotate {
                    joint: 3,
                    axis: X,
                    deg: &[0.0, -100.0, -160.0, -150.0],
                },
                Track::Rotate {
                    joint: 4,
                    axis: X,
                    deg: &[0.0, -110.0, -170.0, -160.0],
                },
                Track::Rotate {
                    joint: 1,
                    axis: X,
                    deg: &[0.0, 20.0, 35.0, 30.0],
                },
            ],
        },
        // Gliding down from the barge: arms out to the sides.
        Clip {
            name: "glide",
            times: &[0.0, 0.5, 1.0],
            tracks: vec![
                Track::Rotate {
                    joint: 3,
                    axis: Z,
                    deg: &[-80.0, -86.0, -80.0],
                },
                Track::Rotate {
                    joint: 4,
                    axis: Z,
                    deg: &[80.0, 86.0, 80.0],
                },
                Track::Rotate {
                    joint: 1,
                    axis: X,
                    deg: &[-25.0, -30.0, -25.0],
                },
                Track::Rotate {
                    joint: 2,
                    axis: X,
                    deg: &[-20.0, -15.0, -20.0],
                },
            ],
        },
        // The winner's dance: both arms up, bouncing.
        Clip {
            name: "victory",
            times: &[0.0, 0.25, 0.5],
            tracks: vec![
                Track::Rotate {
                    joint: 3,
                    axis: X,
                    deg: &[-170.0, -150.0, -170.0],
                },
                Track::Rotate {
                    joint: 4,
                    axis: X,
                    deg: &[-150.0, -170.0, -150.0],
                },
                Track::Offset {
                    joint: 0,
                    by: &[[0.0, 0.0, 0.0], [0.0, 0.18, 0.0], [0.0, 0.0, 0.0]],
                },
            ],
        },
        // Channelling a shield cell: hands together in front, a slight bob.
        Clip {
            name: "channel",
            times: &[0.0, 0.4, 0.8],
            tracks: vec![
                Track::Rotate {
                    joint: 3,
                    axis: X,
                    deg: &[-50.0, -56.0, -50.0],
                },
                Track::Rotate {
                    joint: 4,
                    axis: X,
                    deg: &[-50.0, -56.0, -50.0],
                },
                Track::Rotate {
                    joint: 5,
                    axis: X,
                    deg: &[12.0, 16.0, 12.0],
                },
            ],
        },
        Clip {
            name: "hold",
            times: HOLD_T,
            tracks: vec![Track::Rotate {
                joint: 0,
                axis: X,
                deg: &[0.0, 0.0],
            }],
        },
    ]
}

/// A character in its colours: shirt, pants, hood, pack.
fn character(
    name: &'static str,
    shirt: [u8; 3],
    pants: [u8; 3],
    hood: [u8; 3],
    pack: [u8; 3],
) -> Model {
    Model {
        name,
        palette: vec![
            [236, 196, 158],
            shirt,
            pants,
            [34, 28, 30],
            hood,
            pack,
            [255, 214, 120],
        ],
        joints: character_joints(),
        parts: character_parts(),
        clips: character_clips(),
        tube: None,
    }
}

/// A loot chest: base and a lid on a hinge at the back, which opens.
fn chest() -> Model {
    const WOOD: usize = 0;
    const BAND: usize = 1;
    const GOLD: usize = 2;
    Model {
        name: "chest",
        palette: vec![[120, 74, 40], [60, 52, 60], [245, 196, 70]],
        joints: vec![
            ("base", None, [0.0, 0.0, 0.0]),
            ("lid", Some(0), [0.0, 0.5, -0.32]),
        ],
        parts: vec![
            ([-0.5, 0.0, -0.32], [0.5, 0.5, 0.32], 0, WOOD),
            ([-0.52, 0.18, -0.34], [0.52, 0.26, 0.34], 0, BAND),
            ([-0.5, 0.5, -0.32], [0.5, 0.72, 0.32], 1, WOOD),
            ([-0.52, 0.56, -0.34], [0.52, 0.64, 0.34], 1, BAND),
            ([-0.08, 0.38, 0.32], [0.08, 0.6, 0.37], 1, GOLD),
        ],
        tube: None,
        clips: vec![
            Clip {
                name: "shut",
                times: HOLD_T,
                tracks: vec![Track::Rotate {
                    joint: 1,
                    axis: X,
                    deg: &[0.0, 0.0],
                }],
            },
            Clip {
                name: "open",
                times: &[0.0, 0.18, 0.3],
                tracks: vec![Track::Rotate {
                    joint: 1,
                    axis: X,
                    deg: &[0.0, -118.0, -105.0],
                }],
            },
        ],
    }
}

fn prop(name: &'static str, palette: Vec<[u8; 3]>, parts: Vec<Part>) -> Model {
    Model {
        name,
        palette,
        joints: Vec::new(),
        parts,
        clips: Vec::new(),
        tube: None,
    }
}

/// The lantern barge that carries everyone over the island: a hull, a
/// long balloon, four propellers and a row of lanterns.
fn barge() -> Model {
    const HULL: usize = 0;
    const TRIM: usize = 1;
    const CLOTH: usize = 2;
    const STRIPE: usize = 3;
    const LAMP: usize = 4;
    const DARK: usize = 5;
    prop(
        "barge",
        vec![
            [120, 82, 54],
            [70, 50, 40],
            [228, 222, 206],
            [210, 120, 60],
            [255, 214, 120],
            [40, 34, 40],
        ],
        vec![
            // hull and deck
            ([-1.6, 0.0, -5.0], [1.6, 1.2, 5.0], 0, HULL),
            ([-1.2, -0.4, -4.2], [1.2, 0.0, 4.2], 0, HULL),
            ([-1.7, 1.2, -5.1], [1.7, 1.4, 5.1], 0, TRIM),
            ([-0.9, 1.4, -1.5], [0.9, 2.6, 1.5], 0, TRIM),
            // balloon: a long stack, wider in the middle, with a stripe
            ([-2.6, 4.0, -6.5], [2.6, 7.4, 6.5], 0, CLOTH),
            ([-2.2, 3.6, -7.5], [2.2, 7.8, 7.5], 0, CLOTH),
            ([-1.6, 4.4, -8.3], [1.6, 7.0, 8.3], 0, CLOTH),
            ([-2.65, 5.4, -6.6], [2.65, 6.0, 6.6], 0, STRIPE),
            // fins
            ([-0.15, 6.8, 6.6], [0.15, 8.8, 8.6], 0, STRIPE),
            ([-2.4, 5.5, 7.0], [2.4, 5.8, 8.6], 0, STRIPE),
            // ropes (posts) from hull to balloon
            ([-1.3, 1.4, -3.8], [-1.1, 3.8, -3.6], 0, DARK),
            ([1.1, 1.4, -3.8], [1.3, 3.8, -3.6], 0, DARK),
            ([-1.3, 1.4, 3.6], [-1.1, 3.8, 3.8], 0, DARK),
            ([1.1, 1.4, 3.6], [1.3, 3.8, 3.8], 0, DARK),
            // propellers
            ([-2.4, 0.3, 4.4], [-1.6, 1.1, 4.6], 0, DARK),
            ([1.6, 0.3, 4.4], [2.4, 1.1, 4.6], 0, DARK),
            ([-2.4, 0.3, -4.6], [-1.6, 1.1, -4.4], 0, DARK),
            ([1.6, 0.3, -4.6], [2.4, 1.1, -4.4], 0, DARK),
            // lanterns along the rail
            ([-1.85, 1.5, -3.0], [-1.6, 1.9, -2.6], 0, LAMP),
            ([1.6, 1.5, -3.0], [1.85, 1.9, -2.6], 0, LAMP),
            ([-1.85, 1.5, 0.0], [-1.6, 1.9, 0.4], 0, LAMP),
            ([1.6, 1.5, 0.0], [1.85, 1.9, 0.4], 0, LAMP),
            ([-1.85, 1.5, 3.0], [-1.6, 1.9, 3.4], 0, LAMP),
            ([1.6, 1.5, 3.0], [1.85, 1.9, 3.4], 0, LAMP),
        ],
    )
}

/// The Hush's wall: a smooth open tube, coloured and faded by its look.
fn hush_wall() -> Model {
    Model {
        name: "hush_wall",
        palette: vec![[255, 255, 255]],
        joints: Vec::new(),
        parts: Vec::new(),
        clips: Vec::new(),
        tube: Some(96),
    }
}

/// The six weapons as loot: small voxel props, about 0.8 m long, along +Z.
fn guns() -> Vec<Model> {
    let metal = [70, 76, 88];
    let dark = [36, 34, 40];
    let wood = [120, 80, 50];
    vec![
        prop(
            "gun_wrench",
            vec![metal, dark, [200, 120, 60]],
            vec![
                ([-0.05, 0.0, -0.4], [0.05, 0.08, 0.25], 0, 0),
                ([-0.14, 0.0, 0.2], [0.14, 0.08, 0.42], 0, 0),
                ([-0.05, 0.0, -0.45], [0.05, 0.1, -0.25], 0, 2),
            ],
        ),
        prop(
            "gun_pistol",
            vec![metal, dark, [120, 220, 255]],
            vec![
                ([-0.06, 0.0, -0.1], [0.06, 0.12, 0.32], 0, 0),
                ([-0.05, -0.16, -0.12], [0.05, 0.0, 0.0], 0, 1),
                ([-0.07, 0.05, 0.1], [0.07, 0.15, 0.2], 0, 2),
            ],
        ),
        prop(
            "gun_smg",
            vec![metal, dark, [80, 240, 255]],
            vec![
                ([-0.07, 0.0, -0.3], [0.07, 0.14, 0.4], 0, 0),
                ([-0.05, -0.2, 0.0], [0.05, 0.0, 0.1], 0, 1),
                ([-0.05, -0.14, -0.22], [0.05, 0.0, -0.14], 0, 1),
                ([-0.08, 0.06, -0.05], [0.08, 0.16, 0.25], 0, 2),
            ],
        ),
        prop(
            "gun_scatter",
            vec![wood, dark, [255, 160, 70]],
            vec![
                ([-0.08, 0.0, -0.45], [0.08, 0.14, 0.0], 0, 0),
                ([-0.1, 0.02, 0.0], [0.1, 0.16, 0.5], 0, 1),
                ([-0.13, 0.0, 0.42], [0.13, 0.18, 0.55], 0, 2),
            ],
        ),
        prop(
            "gun_rifle",
            vec![wood, metal, [255, 210, 90]],
            vec![
                ([-0.07, 0.0, -0.55], [0.07, 0.13, -0.05], 0, 0),
                ([-0.05, 0.03, -0.05], [0.05, 0.11, 0.65], 0, 1),
                ([-0.06, 0.13, -0.1], [0.06, 0.22, 0.2], 0, 2),
            ],
        ),
        prop(
            "gun_mortar",
            vec![[90, 70, 100], dark, [255, 120, 200]],
            vec![
                ([-0.13, 0.0, -0.35], [0.13, 0.26, 0.35], 0, 0),
                ([-0.16, 0.02, 0.3], [0.16, 0.28, 0.45], 0, 2),
                ([-0.08, -0.15, -0.1], [0.08, 0.0, 0.05], 0, 1),
            ],
        ),
    ]
}

/// Lanternfall G5: each place's props, and the island's night growth.
fn dressing() -> Vec<Model> {
    let wood = [118, 80, 50];
    let dark_wood = [78, 52, 34];
    let iron = [58, 60, 70];
    let stone = [120, 124, 136];
    let moss = [70, 104, 66];
    vec![
        // Lantern Docks: barrels, a rowing boat, a mooring post.
        prop(
            "barrel",
            vec![wood, iron],
            vec![
                ([-0.32, 0.0, -0.32], [0.32, 0.9, 0.32], 0, 0),
                ([-0.36, 0.12, -0.36], [0.36, 0.2, 0.36], 0, 1),
                ([-0.36, 0.7, -0.36], [0.36, 0.78, 0.36], 0, 1),
            ],
        ),
        prop(
            "boat",
            vec![wood, dark_wood, [200, 190, 170]],
            vec![
                ([-0.8, 0.0, -2.2], [0.8, 0.35, 2.2], 0, 1),
                ([-0.9, 0.35, -2.4], [0.9, 0.7, 2.4], 0, 0),
                ([-0.7, 0.35, -2.0], [0.7, 0.62, 2.0], 0, 1),
                ([-0.6, 0.6, -0.2], [0.6, 0.7, 0.2], 0, 0),
                ([-0.05, 0.7, 0.8], [0.05, 2.8, 0.9], 0, 1),
                ([-0.04, 1.1, 0.9], [0.04, 2.6, 2.0], 0, 2),
            ],
        ),
        prop(
            "post",
            vec![dark_wood, iron],
            vec![
                ([-0.14, 0.0, -0.14], [0.14, 1.1, 0.14], 0, 0),
                ([-0.18, 0.9, -0.18], [0.18, 1.0, 0.18], 0, 1),
            ],
        ),
        // Glasshouse Observatory: a brass telescope on a tripod.
        prop(
            "telescope",
            vec![[200, 160, 80], iron, [120, 200, 255]],
            vec![
                ([-0.4, 0.0, -0.4], [-0.3, 1.2, -0.3], 0, 1),
                ([0.3, 0.0, -0.4], [0.4, 1.2, -0.3], 0, 1),
                ([-0.05, 0.0, 0.3], [0.05, 1.2, 0.4], 0, 1),
                ([-0.3, 1.2, -0.3], [0.3, 1.35, 0.3], 0, 1),
                ([-0.16, 1.35, -0.9], [0.16, 1.67, 0.9], 0, 0),
                ([-0.2, 1.31, 0.9], [0.2, 1.71, 1.1], 0, 0),
                ([-0.14, 1.37, 1.1], [0.14, 1.65, 1.12], 0, 2),
            ],
        ),
        // Rust Hangars: fuel drums and a stack of tyres.
        prop(
            "drum",
            vec![[170, 70, 46], [90, 40, 30], [220, 200, 80]],
            vec![
                ([-0.3, 0.0, -0.3], [0.3, 0.95, 0.3], 0, 0),
                ([-0.32, 0.3, -0.32], [0.32, 0.36, 0.32], 0, 1),
                ([-0.32, 0.65, -0.32], [0.32, 0.71, 0.32], 0, 1),
                ([-0.1, 0.4, 0.3], [0.1, 0.6, 0.32], 0, 2),
            ],
        ),
        prop(
            "tyres",
            vec![[34, 32, 36], [60, 58, 62]],
            vec![
                ([-0.45, 0.0, -0.45], [0.45, 0.22, 0.45], 0, 0),
                ([-0.42, 0.22, -0.42], [0.42, 0.44, 0.42], 0, 1),
                ([-0.45, 0.44, -0.45], [0.45, 0.66, 0.45], 0, 0),
            ],
        ),
        // Sunken Chapel: gravestones, a broken pillar, a candle.
        prop(
            "grave",
            vec![stone, moss],
            vec![
                ([-0.35, 0.0, -0.1], [0.35, 0.75, 0.1], 0, 0),
                ([-0.25, 0.75, -0.1], [0.25, 0.9, 0.1], 0, 0),
                ([-0.36, 0.0, -0.11], [0.36, 0.15, 0.11], 0, 1),
            ],
        ),
        prop(
            "pillar",
            vec![stone, moss, [100, 104, 116]],
            vec![
                ([-0.45, 0.0, -0.45], [0.45, 0.3, 0.45], 0, 2),
                ([-0.32, 0.3, -0.32], [0.32, 2.4, 0.32], 0, 0),
                ([-0.36, 2.4, -0.36], [0.2, 2.7, 0.28], 0, 2),
                ([-0.33, 0.3, -0.33], [0.33, 0.5, 0.33], 0, 1),
            ],
        ),
        prop(
            "candle",
            vec![[236, 228, 210], [255, 200, 110]],
            vec![
                ([-0.06, 0.0, -0.06], [0.06, 0.3, 0.06], 0, 0),
                ([-0.025, 0.3, -0.025], [0.025, 0.38, 0.025], 0, 1),
            ],
        ),
        // Signal Vault: a standard on a pole.
        prop(
            "banner",
            vec![iron, [110, 70, 170], [235, 200, 110]],
            vec![
                ([-0.06, 0.0, -0.06], [0.06, 3.4, 0.06], 0, 0),
                ([0.06, 1.6, -0.03], [1.0, 3.2, 0.03], 0, 1),
                ([0.06, 2.9, -0.04], [1.0, 3.0, 0.04], 0, 2),
                ([0.4, 2.1, -0.04], [0.7, 2.5, 0.04], 0, 2),
            ],
        ),
        // The island's night growth: glowing mushrooms, bushes, flowers.
        prop(
            "mushrooms",
            vec![[220, 230, 240], [120, 230, 255], [70, 60, 80]],
            vec![
                ([-0.05, 0.0, -0.05], [0.05, 0.28, 0.05], 0, 0),
                ([-0.18, 0.26, -0.18], [0.18, 0.36, 0.18], 0, 1),
                ([0.22, 0.0, 0.12], [0.28, 0.16, 0.18], 0, 0),
                ([0.14, 0.15, 0.04], [0.36, 0.22, 0.26], 0, 1),
                ([-0.3, 0.0, 0.2], [-0.25, 0.12, 0.25], 0, 0),
                ([-0.36, 0.11, 0.14], [-0.19, 0.17, 0.31], 0, 1),
            ],
        ),
        prop(
            "bush",
            vec![[46, 92, 58], [36, 74, 50], [62, 112, 70]],
            vec![
                ([-0.6, 0.0, -0.5], [0.6, 0.6, 0.5], 0, 0),
                ([-0.4, 0.4, -0.35], [0.45, 0.9, 0.4], 0, 2),
                ([0.2, 0.0, -0.7], [0.8, 0.45, -0.1], 0, 1),
            ],
        ),
        prop(
            "flowers",
            vec![[60, 110, 60], [240, 190, 90], [200, 120, 220]],
            vec![
                ([-0.4, 0.0, -0.02], [-0.38, 0.3, 0.0], 0, 0),
                ([-0.45, 0.3, -0.06], [-0.33, 0.38, 0.06], 0, 1),
                ([0.1, 0.0, 0.2], [0.12, 0.25, 0.22], 0, 0),
                ([0.05, 0.25, 0.15], [0.17, 0.33, 0.27], 0, 2),
                ([0.3, 0.0, -0.3], [0.32, 0.35, -0.28], 0, 0),
                ([0.25, 0.35, -0.35], [0.37, 0.43, -0.23], 0, 1),
            ],
        ),
    ]
}

fn loot_bits() -> Vec<Model> {
    vec![
        prop(
            "cell",
            vec![[120, 200, 255], [40, 90, 160]],
            vec![
                ([-0.12, 0.0, -0.12], [0.12, 0.36, 0.12], 0, 0),
                ([-0.15, 0.08, -0.15], [0.15, 0.28, 0.15], 0, 1),
            ],
        ),
        prop(
            "ammo",
            vec![[200, 170, 70], [90, 76, 40]],
            vec![
                ([-0.2, 0.0, -0.14], [0.2, 0.2, 0.14], 0, 0),
                ([-0.21, 0.08, -0.15], [0.21, 0.12, 0.15], 0, 1),
            ],
        ),
    ]
}

/// A lantern post: a pole, an arm and a hanging lamp (the light itself
/// is a point light the game places at the lamp).
fn lantern_post() -> Model {
    prop(
        "lantern_post",
        vec![[48, 40, 46], [255, 214, 120], [90, 70, 50]],
        vec![
            ([-0.07, 0.0, -0.07], [0.07, 2.6, 0.07], 0, 0),
            ([-0.18, 0.0, -0.18], [0.18, 0.15, 0.18], 0, 0),
            ([-0.04, 2.5, -0.04], [0.7, 2.58, 0.04], 0, 0),
            ([0.52, 2.08, -0.13], [0.78, 2.38, 0.13], 0, 1),
            ([0.5, 2.38, -0.15], [0.8, 2.44, 0.15], 0, 2),
        ],
    )
}

fn tree() -> Model {
    prop(
        "tree",
        vec![[92, 62, 44], [46, 104, 70], [36, 84, 60], [58, 124, 82]],
        vec![
            ([-0.18, 0.0, -0.18], [0.18, 1.4, 0.18], 0, 0),
            ([-0.9, 1.1, -0.9], [0.9, 1.9, 0.9], 0, 2),
            ([-0.7, 1.8, -0.7], [0.7, 2.5, 0.7], 0, 1),
            ([-0.45, 2.4, -0.45], [0.45, 3.0, 0.45], 0, 3),
        ],
    )
}

fn rock() -> Model {
    prop(
        "rock",
        vec![[104, 110, 124], [84, 90, 104]],
        vec![
            ([-0.6, 0.0, -0.5], [0.55, 0.55, 0.5], 0, 0),
            ([-0.35, 0.5, -0.3], [0.3, 0.85, 0.32], 0, 1),
            ([0.3, 0.0, 0.1], [0.75, 0.35, 0.55], 0, 1),
        ],
    )
}

fn crate_box() -> Model {
    prop(
        "crate",
        vec![[150, 104, 62], [96, 64, 40]],
        vec![
            ([-0.45, 0.0, -0.45], [0.45, 0.9, 0.45], 0, 0),
            ([-0.47, 0.0, -0.47], [-0.37, 0.9, -0.37], 0, 1),
            ([0.37, 0.0, -0.47], [0.47, 0.9, -0.37], 0, 1),
            ([-0.47, 0.0, 0.37], [-0.37, 0.9, 0.47], 0, 1),
            ([0.37, 0.0, 0.37], [0.47, 0.9, 0.47], 0, 1),
            ([-0.47, 0.82, -0.47], [0.47, 0.92, 0.47], 0, 1),
        ],
    )
}

fn models() -> Vec<Model> {
    vec![
        // The player: lantern-orange shirt, teal hood.
        character(
            "keeper",
            [236, 132, 52],
            [52, 58, 84],
            [40, 150, 150],
            [110, 82, 56],
        ),
        // Bots, in four colourways.
        character(
            "folk_moss",
            [92, 160, 84],
            [60, 54, 46],
            [62, 102, 56],
            [96, 74, 52],
        ),
        character(
            "folk_dusk",
            [120, 90, 190],
            [44, 44, 70],
            [78, 58, 140],
            [90, 70, 60],
        ),
        character(
            "folk_ember",
            [200, 70, 70],
            [60, 40, 44],
            [140, 44, 50],
            [100, 72, 50],
        ),
        character(
            "folk_tide",
            [64, 140, 210],
            [40, 52, 76],
            [40, 92, 150],
            [86, 70, 58],
        ),
        chest(),
        lantern_post(),
        tree(),
        rock(),
        crate_box(),
        barge(),
        hush_wall(),
    ]
    .into_iter()
    .chain(guns())
    .chain(loot_bits())
    .chain(dressing())
    .collect()
}

/// The open tube: `n` segments around, two rings of vertices, outward
/// normals (the material is double-sided, so it shows from inside too).
fn tube_geometry(n: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<u32>) {
    let (mut pos, mut nrm, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        for y in [0.0f32, 1.0] {
            pos.extend_from_slice(&[c, y, s]);
            nrm.extend_from_slice(&[c, 0.0, s]);
            uv.extend_from_slice(&[0.5, 0.5]);
        }
    }
    for i in 0..n as u32 {
        let b = i * 2;
        idx.extend_from_slice(&[b, b + 2, b + 1, b + 1, b + 2, b + 3]);
    }
    (pos, nrm, uv, idx)
}

// ---------------------------------------------------------------
// The builder (glTF 2.0 binary).
// ---------------------------------------------------------------

#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl Builder {
    fn view(&mut self, bytes: &[u8]) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        self.views.push(format!(
            r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{}}}"#,
            bytes.len()
        ));
        self.views.len() - 1
    }

    fn accessor(&mut self, bytes: &[u8], ctype: u32, count: usize, ty: &str, extra: &str) -> usize {
        let view = self.view(bytes);
        self.accessors.push(format!(
            r#"{{"bufferView":{view},"componentType":{ctype},"count":{count},"type":"{ty}"{extra}}}"#
        ));
        self.accessors.len() - 1
    }

    fn floats(&mut self, data: &[f32], per: usize, ty: &str, extra: &str) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.accessor(&bytes, 5126, data.len() / per, ty, extra)
    }
}

fn nums(xs: &[f32]) -> String {
    xs.iter()
        .map(|x| format!("{x}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn quat_axis(axis: [f32; 3], deg: f32) -> [f32; 4] {
    let h = deg.to_radians() / 2.0;
    [
        axis[0] * h.sin(),
        axis[1] * h.sin(),
        axis[2] * h.sin(),
        h.cos(),
    ]
}

fn build(m: &Model) -> Vec<u8> {
    let skinned = !m.joints.is_empty();
    let (mut pos, mut nrm, mut uv) = (Vec::new(), Vec::new(), Vec::new());
    let (mut joints, mut weights, mut idx) = (Vec::<u16>::new(), Vec::new(), Vec::<u32>::new());
    if let Some(n) = m.tube {
        (pos, nrm, uv, idx) = tube_geometry(n);
    }
    for &(lo, hi, joint, cell) in &m.parts {
        let u = (cell as f32 + 0.5) / m.palette.len() as f32;
        let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
            (
                [1.0, 0.0, 0.0],
                [
                    [hi[0], lo[1], hi[2]],
                    [hi[0], lo[1], lo[2]],
                    [hi[0], hi[1], lo[2]],
                    [hi[0], hi[1], hi[2]],
                ],
            ),
            (
                [-1.0, 0.0, 0.0],
                [
                    [lo[0], lo[1], lo[2]],
                    [lo[0], lo[1], hi[2]],
                    [lo[0], hi[1], hi[2]],
                    [lo[0], hi[1], lo[2]],
                ],
            ),
            (
                [0.0, 1.0, 0.0],
                [
                    [lo[0], hi[1], hi[2]],
                    [hi[0], hi[1], hi[2]],
                    [hi[0], hi[1], lo[2]],
                    [lo[0], hi[1], lo[2]],
                ],
            ),
            (
                [0.0, -1.0, 0.0],
                [
                    [lo[0], lo[1], lo[2]],
                    [hi[0], lo[1], lo[2]],
                    [hi[0], lo[1], hi[2]],
                    [lo[0], lo[1], hi[2]],
                ],
            ),
            (
                [0.0, 0.0, 1.0],
                [
                    [lo[0], lo[1], hi[2]],
                    [hi[0], lo[1], hi[2]],
                    [hi[0], hi[1], hi[2]],
                    [lo[0], hi[1], hi[2]],
                ],
            ),
            (
                [0.0, 0.0, -1.0],
                [
                    [hi[0], lo[1], lo[2]],
                    [lo[0], lo[1], lo[2]],
                    [lo[0], hi[1], lo[2]],
                    [hi[0], hi[1], lo[2]],
                ],
            ),
        ];
        for (n, corners) in faces {
            let base = (pos.len() / 3) as u32;
            for c in corners {
                pos.extend_from_slice(&c);
                nrm.extend_from_slice(&n);
                uv.extend_from_slice(&[u, 0.5]);
                joints.extend_from_slice(&[joint, 0, 0, 0]);
                weights.extend_from_slice(&[1.0, 0.0, 0.0, 0.0]);
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in pos.chunks(3) {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }

    let mut b = Builder::default();
    let a_pos = b.floats(
        &pos,
        3,
        "VEC3",
        &format!(r#","min":[{}],"max":[{}]"#, nums(&min), nums(&max)),
    );
    let a_nrm = b.floats(&nrm, 3, "VEC3", "");
    let a_uv = b.floats(&uv, 2, "VEC2", "");
    let idx_bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
    let a_idx = b.accessor(&idx_bytes, 5125, idx.len(), "SCALAR", "");
    let mut attributes = format!(r#""POSITION":{a_pos},"NORMAL":{a_nrm},"TEXCOORD_0":{a_uv}"#);

    let mut nodes = Vec::new();
    let mut extra = String::new();
    if skinned {
        let joint_bytes: Vec<u8> = joints.iter().flat_map(|j| j.to_le_bytes()).collect();
        let a_joints = b.accessor(&joint_bytes, 5123, joints.len() / 4, "VEC4", "");
        let a_weights = b.floats(&weights, 4, "VEC4", "");
        let _ = write!(
            attributes,
            r#","JOINTS_0":{a_joints},"WEIGHTS_0":{a_weights}"#
        );
        // Bind poses are translations by each joint's world position.
        let world = |j: usize| {
            let mut w = [0.0f32; 3];
            let mut at = Some(j);
            while let Some(k) = at {
                let (_, parent, t) = m.joints[k];
                for i in 0..3 {
                    w[i] += t[i];
                }
                at = parent;
            }
            w
        };
        let mut ibm = Vec::new();
        for j in 0..m.joints.len() {
            let w = world(j);
            ibm.extend_from_slice(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
            ibm.extend_from_slice(&[-w[0], -w[1], -w[2], 1.0]);
        }
        let a_ibm = b.floats(&ibm, 16, "MAT4", "");
        // Joint j is node j + 1 (node 0 is the mesh).
        let mut anims = Vec::new();
        for clip in &m.clips {
            let t_last = *clip.times.last().unwrap();
            let a_t = b.floats(
                clip.times,
                1,
                "SCALAR",
                &format!(r#","min":[0],"max":[{t_last}]"#),
            );
            let mut samplers = Vec::new();
            let mut channels = Vec::new();
            for track in &clip.tracks {
                let (joint, path, data, per, ty) = match track {
                    Track::Rotate { joint, axis, deg } => {
                        assert_eq!(deg.len(), clip.times.len(), "{} {}", m.name, clip.name);
                        let rot: Vec<f32> = deg.iter().flat_map(|d| quat_axis(*axis, *d)).collect();
                        (*joint, "rotation", rot, 4, "VEC4")
                    }
                    Track::Offset { joint, by } => {
                        assert_eq!(by.len(), clip.times.len(), "{} {}", m.name, clip.name);
                        let rest = m.joints[*joint].2;
                        let t: Vec<f32> = by
                            .iter()
                            .flat_map(|o| [rest[0] + o[0], rest[1] + o[1], rest[2] + o[2]])
                            .collect();
                        (*joint, "translation", t, 3, "VEC3")
                    }
                };
                let a_out = b.floats(&data, per, ty, "");
                samplers.push(format!(
                    r#"{{"input":{a_t},"output":{a_out},"interpolation":"LINEAR"}}"#
                ));
                channels.push(format!(
                    r#"{{"sampler":{},"target":{{"node":{},"path":"{path}"}}}}"#,
                    samplers.len() - 1,
                    joint + 1
                ));
            }
            anims.push(format!(
                r#"{{"name":"{}","samplers":[{}],"channels":[{}]}}"#,
                clip.name,
                samplers.join(","),
                channels.join(",")
            ));
        }
        nodes.push(format!(r#"{{"name":"{}","mesh":0,"skin":0}}"#, m.name));
        for (j, (name, _, t)) in m.joints.iter().enumerate() {
            let children: Vec<String> = m
                .joints
                .iter()
                .enumerate()
                .filter(|(_, (_, p, _))| *p == Some(j))
                .map(|(c, _)| (c + 1).to_string())
                .collect();
            let mut node = format!(r#"{{"name":"{name}","translation":[{}]"#, nums(t));
            if !children.is_empty() {
                let _ = write!(node, r#","children":[{}]"#, children.join(","));
            }
            node.push('}');
            nodes.push(node);
        }
        let joint_nodes: Vec<String> = (1..=m.joints.len()).map(|n| n.to_string()).collect();
        let _ = write!(
            extra,
            r#""skins":[{{"joints":[{}],"inverseBindMatrices":{a_ibm},"skeleton":1}}],"animations":[{}],"#,
            joint_nodes.join(","),
            anims.join(",")
        );
    } else {
        nodes.push(format!(r#"{{"name":"{}","mesh":0}}"#, m.name));
    }
    let scene_nodes = if skinned { "0,1" } else { "0" };

    let png = palette_png(&m.palette);
    let img_view = b.view(&png);
    let json = format!(
        concat!(
            r#"{{"asset":{{"version":"2.0","generator":"twe tests/gen_lanternfall_models.rs"}},"#,
            r#""scene":0,"scenes":[{{"nodes":[{scene}]}}],"#,
            r#""nodes":[{nodes}],"#,
            r#""meshes":[{{"name":"{name}","primitives":[{{"attributes":{{{attrs}}},"indices":{i},"material":0}}]}}],"#,
            "{extra}",
            r#""materials":[{{"name":"palette","pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}},"metallicFactor":0,"roughnessFactor":0.85}}{double}}}],"#,
            r#""textures":[{{"source":0,"sampler":0}}],"#,
            r#""samplers":[{{"magFilter":9728,"minFilter":9984}}],"#,
            r#""images":[{{"bufferView":{img},"mimeType":"image/png"}}],"#,
            r#""buffers":[{{"byteLength":{len}}}],"#,
            r#""bufferViews":[{views}],"#,
            r#""accessors":[{accessors}]}}"#
        ),
        scene = scene_nodes,
        nodes = nodes.join(","),
        name = m.name,
        attrs = attributes,
        i = a_idx,
        extra = extra,
        double = if m.tube.is_some() {
            r#","doubleSided":true"#
        } else {
            ""
        },
        img = img_view,
        len = b.bin.len().next_multiple_of(4),
        views = b.views.join(","),
        accessors = b.accessors.join(","),
    );
    glb(&json, &b.bin)
}

fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut j = json.as_bytes().to_vec();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let total = 12 + 8 + j.len() + 8 + b.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&j);
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&b);
    out
}

/// The palette as an RGB PNG, CELL px per colour, deflate "stored" blocks.
fn palette_png(palette: &[[u8; 3]]) -> Vec<u8> {
    let (w, h) = (CELL * palette.len(), CELL);
    let mut row = vec![0u8];
    for x in 0..w {
        row.extend_from_slice(&palette[x / CELL]);
    }
    let raw = row.repeat(h);
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(&b"IHDR"[..], &ihdr[..]), (b"IDAT", &z), (b"IEND", &[])] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(data);
        png.extend_from_slice(&crc_input);
        png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
    png
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn lanternfall_models_exist_and_load() {
    let dir = Path::new("examples/lanternfall/assets");
    std::fs::create_dir_all(dir).expect("assets dir");
    for m in models() {
        let path = dir.join(format!("{}.glb", m.name));
        let bytes = build(&m);
        if !path.exists() {
            std::fs::write(&path, &bytes).expect("write model");
        }
        let on_disk = std::fs::read(&path).expect("read model");
        assert_eq!(on_disk, bytes, "{} differs from its recipe", path.display());
        let (doc, _, images) =
            gltf::import_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", m.name));
        assert_eq!(images.len(), 1, "{}", m.name);
        let clips: Vec<String> = doc
            .animations()
            .filter_map(|a| a.name().map(str::to_string))
            .collect();
        let want: Vec<String> = m.clips.iter().map(|c| c.name.to_string()).collect();
        assert_eq!(clips, want, "{}", m.name);
        assert_eq!(
            doc.skins().count(),
            usize::from(!m.joints.is_empty()),
            "{}",
            m.name
        );
        twec::kernel::render::parse_glb_bytes(&bytes)
            .unwrap_or_else(|e| panic!("engine loads {}: {e}", m.name));
    }
}
