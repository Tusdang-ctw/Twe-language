//! LLM grounding material for Twe.
//!
//! Twe is a custom language; a model's prior knowledge of Python / Lua /
//! GDScript actively misleads it on the details. Without grounding, a model
//! connected to `twec mcp` (or driving Twe Studio) sees only mechanical tools
//! and writes generic pseudo-code. This module is the single source of truth
//! that fixes that, surfaced three ways:
//!
//!   - [`INSTRUCTIONS`]   — concise primer for the MCP `initialize` reply's
//!     `instructions` field (hosts inject it into the model's system prompt)
//!     and for Twe Studio's in-app AI system prompt.
//!   - [`guide`]          — the full cheatsheet ([`docs/llm-primer.md`]),
//!     served as the `twe://guide` MCP resource and `twec primer --full`.
//!   - [`EXAMPLES`]       — curated, verified few-shot programs, served as
//!     `twe://examples/<name>` resources.
//!
//! Keeping the canonical text in `docs/llm-primer.md` (embedded via
//! `include_str!`) means humans and models read the same document and it can
//! never drift from what ships in the binary.

/// Concise, information-dense primer. Goes in the MCP `instructions` field and
/// at the head of Twe Studio's AI prompt. Must stand alone: a model that reads
/// only this should already avoid the dominant Twe authoring errors.
pub const INSTRUCTIONS: &str = "\
You are writing Twe — a game-first scripting language (NOT Python/Lua/GDScript, \
though it is indentation-based). Game concepts are keywords. Ground yourself on \
THESE rules; do not assume behaviour from other languages.\n\
\n\
GOLDEN RULES\n\
- Never invent stdlib functions. Call `stdlib_lookup`/`stdlib_list` to confirm a \
name and its params exist (~360 builtins, ~50 categories). Guessing names is the #1 error.\n\
- Always run the `verify` tool on your output and apply its structured `fix` \
patches until it reports zero errors before claiming done. Verified-clean is the contract.\n\
- Drawing (`rect`/`circle`/`text`/`sprite`) is ONLY legal inside an `on render():` \
handler. Mutate state in `every` / `on update(dt)`.\n\
- `-> state_name` transitions are ONLY legal inside a `state` block; code after one is dead.\n\
- Read the `twe://guide` resource and `twe://examples/<name>` for full syntax + worked programs.\n\
\n\
CORE BLOCK KEYWORDS: scene, entity, state, dialogue, visual, particles.\n\
\n\
SEMANTICS THAT DIFFER FROM PYTHON\n\
- `let` = immutable, `var` = mutable. 4-space indent; never mix tabs/spaces.\n\
- ONLY `false` is falsy. `0`, `\"\"`, `nil`, [] are all truthy. `and`/`or` return an \
operand (not a strict bool), so `count or default` yields `count` even when count==0.\n\
- `%` is the percent-literal suffix (`5%`), NOT modulo — use `math.mod(a,b)`.\n\
- Keyword args use `name: value` and must follow positional args, \
e.g. `rect(at: (10,20), size: (100,50), color: color.red)`.\n\
- Literals: tuples/vectors `(x, y)` (with `.x`/`.y` and tuple math); ranges `10..15` \
and `0..<n`; units `0.5s 200ms 100px 90deg` (dimension-checked); interpolation `\"hi {name}\"`.\n\
\n\
EVENTS: `on update(dt):` (dt is fixed 1/60s), `on render():`, `on key_press.space:` \
(edge), `on hp < 20%:` (predicate, false→true), `every 150ms:` (timed, in a state).\n\
STATE MACHINES: a container sets `initial: <state>`; each `state X:` holds `on enter:`/\
`on exit:`, handlers, `every` clocks; `-> Y` switches state.\n\
ENTITIES: `entity Slime extends Enemy:` with fields, `function m(...):` (implicit `self`), \
lifecycle handlers; `spawn Slime at (x, y)`, `for s in entities.of(Slime):`.";

/// The full Markdown cheatsheet (`docs/llm-primer.md`). Served as the
/// `twe://guide` resource and printed by `twec primer --full`.
pub fn guide() -> &'static str {
    include_str!("../docs/llm-primer.md")
}

/// A curated few-shot program: a stable id, a one-line description, and the
/// verified source. Embedded so the set ships with the binary and never
/// depends on the client's working directory.
pub struct Example {
    pub name: &'static str,
    pub description: &'static str,
    pub source: &'static str,
}

/// Curated, verified examples surfaced as `twe://examples/<name>`. Kept small
/// and high-signal — over-prompting with many examples degrades DSL accuracy,
/// so this is a hand-picked pair covering the two dominant shapes (a scene with
/// a state machine, and an entity-driven game).
pub const EXAMPLES: &[Example] = &[
    Example {
        name: "snake",
        description: "Scene + state machine: grid movement, food, game-over/restart.",
        source: include_str!("../examples/snake.twe"),
    },
    Example {
        name: "pong",
        description: "Classic Pong: paddles, ball physics, scoring, simple AI.",
        source: include_str!("../examples/pong.twe"),
    },
];

/// Look up a curated example by name.
pub fn example(name: &str) -> Option<&'static Example> {
    EXAMPLES.iter().find(|e| e.name == name)
}
