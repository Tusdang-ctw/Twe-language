//! Lanternfall G0: `spawn` as an expression — `let b = spawn Bullet at p`
//! spawns as the statement does and is the new entity, so a game can
//! set its fields (a bullet's direction, a building's size) right away.

fn run(src: &str) -> twec::value::Env {
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("run");
    env
}

#[test]
fn spawn_is_the_new_entity() {
    let env = run(
        "entity Shot:\n    var pos = vec3(0, 0, 0)\n    var dir = vec3(0, 0, 1)\n\
         let s = spawn Shot at vec3(1, 2, 3)\ns.dir = vec3(1, 0, 0)\n\
         for x in entities.of(Shot):\n    print(\"{x.pos.y} {x.dir.x}\")\n\
         print(entities.count(Shot))\n",
    );
    assert_eq!(env.out, "2.0 1.0\n1\n");
}

#[test]
fn spawned_entities_can_go_straight_into_lists_and_calls() {
    let env = run(
        "entity Box:\n    var pos = vec3(0, 0, 0)\n    var size = 1.0\n\
         function grow(b, s):\n    b.size = s\n    return b\n\
         let boxes = [spawn Box at vec3(i, 0, 0) for i in 0..<3]\n\
         grow(spawn Box, 9.0)\n\
         print(\"{boxes.length} {entities.count(Box)}\")\n\
         var total = 0.0\nfor b in entities.of(Box):\n    total += b.size\nprint(total)\n",
    );
    assert_eq!(env.out, "3 4\n12.0\n");
}

#[test]
fn the_statement_form_is_unchanged_and_round_trips() {
    let src = "entity Box:\n    var pos = vec3(0, 0, 0)\nspawn Box at vec3(1, 0, 0)\nlet b = spawn Box at vec3(2, 0, 0)\n";
    let program = twec::parser::parse(&twec::lexer::lex(src).unwrap()).unwrap();
    let printed = twec::printer::print_program(&program);
    assert!(printed.contains("spawn Box at vec3(1, 0, 0)\n"), "{printed}");
    assert!(printed.contains("let b = spawn Box at vec3(2, 0, 0)\n"), "{printed}");
    let again = twec::parser::parse(&twec::lexer::lex(&printed).unwrap()).unwrap();
    assert_eq!(twec::printer::print_program(&again), printed);
}

/// Lanternfall G0: strings have `.length` (characters), as lists do.
#[test]
fn strings_have_a_length() {
    let env = run("let s = \"Lantern Docks\"\nprint(s.length)\nprint(\"é∑\".length)\n");
    assert_eq!(env.out, "13\n2\n");
}
