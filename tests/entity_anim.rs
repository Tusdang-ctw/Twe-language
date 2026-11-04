//! Lanternfall E4: per-entity skeletal animation, script side.
//!
//! `mesh_anim.*` takes an entity as well as a mesh path: an entity's own
//! animation lives on the entity, advances with `mesh_anim.advance`, and
//! poses its look apart from every other entity drawing the same mesh
//! (the pixel check is `tests/kernel_render.rs::
//! entities_sharing_a_mesh_animate_apart`).

/// Run `src`, then `frames` ticks and one 3D render frame; return the env.
fn run(src: &str, frames: u32) -> twec::value::Env {
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    for _ in 0..frames {
        twec::eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
    }
    twec::eval::render_frame3d(&mut env).expect("render");
    env
}

fn probe(env: &mut twec::value::Env, src: &str) -> String {
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    env.out.clear();
    twec::eval::run_top_level(env, &program).expect("probe");
    env.out.trim().to_string()
}

const BOTS: &str = r#"
entity Bot:
    var pos = vec3(0, 0, 0)
    var clip = "idle"
    look:
        mesh: "assets/bot.glb"
    function update(dt):
        mesh_anim.play(self, clip, true)

spawn Bot at vec3(0, 0, 0)
spawn Bot at vec3(3, 0, 0)
for b in entities.of(Bot):
    if b.pos.x > 1.0:
        b.clip = "run"

on update(dt):
    mesh_anim.advance(dt)
"#;

#[test]
fn each_entity_keeps_its_own_clip_and_clock() {
    let mut env = run(BOTS, 30);
    let out = probe(
        &mut env,
        "for b in entities.of(Bot):\n    let a = mesh_anim.current(b)\n    print(\"{a.clip} {math.floor(a.time * 10)}\")\n",
    );
    let mut lines: Vec<&str> = out.lines().collect();
    lines.sort();
    // Each plays its clip; both advanced (the first tick's advance runs
    // before the entities' first `play`, so 29 ticks of time).
    assert_eq!(lines, vec!["idle 4", "run 4"], "{out}");
    // The mesh's shared animation is untouched.
    assert_eq!(
        probe(
            &mut env,
            "print(mesh_anim.current(\"assets/bot.glb\").clip)"
        ),
        ""
    );
}

#[test]
fn animated_entities_queue_their_own_pose() {
    let env = run(BOTS, 5);
    // Two draws, each pointing at its own snapshot.
    let anims: Vec<u32> = env.render_queue3d.iter().map(|d| d.anim).collect();
    let mut sorted = anims.clone();
    sorted.sort();
    assert_eq!(sorted, vec![1, 2], "{anims:?}");
    let mut clips: Vec<String> = env.instance_anims.iter().map(|a| a.clip.clone()).collect();
    clips.sort();
    assert_eq!(clips, vec!["idle".to_string(), "run".to_string()]);
}

#[test]
fn changing_an_entitys_clip_crossfades_only_that_entity() {
    let mut env = run(BOTS, 10);
    probe(
        &mut env,
        "for b in entities.of(Bot):\n    if b.pos.x < 1.0:\n        mesh_anim.play(b, \"walk\", true)\n",
    );
    twec::eval::render_frame3d(&mut env).expect("render");
    let mut fading: Vec<(String, Option<String>)> = env
        .instance_anims
        .iter()
        .map(|a| (a.clip.clone(), a.blend_clip.clone()))
        .collect();
    fading.sort();
    // The first bot crossfades idle -> walk; the second still just runs.
    // (Its `update` would set "idle" again next tick; no tick has run.)
    assert_eq!(
        fading,
        vec![
            ("run".to_string(), None),
            ("walk".to_string(), Some("idle".to_string())),
        ]
    );
}

#[test]
fn stopping_an_entity_returns_it_to_the_shared_animation() {
    let src = "entity Bot:\n    var pos = vec3(0, 0, 0)\n    look:\n        mesh: \"assets/bot.glb\"\n\
               spawn Bot\nfor b in entities.of(Bot):\n    mesh_anim.play(b, \"run\", true)\n    mesh_anim.stop(b)\n";
    let env = run(src, 1);
    assert_eq!(env.render_queue3d.len(), 1);
    assert_eq!(
        env.render_queue3d[0].anim, 0,
        "back on the mesh's animation"
    );
    assert!(env.instance_anims.is_empty());
}

#[test]
fn a_non_entity_non_path_handle_says_what_to_pass() {
    let program =
        twec::parser::parse(&twec::lexer::lex("mesh_anim.play(3.5, \"run\", true)\n").unwrap())
            .unwrap();
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    let err = twec::eval::run_top_level(&mut env, &program).expect_err("bad handle");
    let help = err.help.unwrap_or_default();
    assert!(help.contains("self"), "{help}");
}
