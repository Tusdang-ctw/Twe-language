//! Phase 9 session 9: subset typechecker for `visual` block bodies.
//!
//! Per Example 5's implied decisions in `docs/01-examples.md`:
//! > The Twe subset inside `visual` is restricted: no allocations,
//! > no loops without compile-time bounds, no calling host code.
//!
//! This module walks every `visual` declaration in the program and
//! reports errors for constructs that the WGSL fragment-shader
//! compiler (Phase 9 session 10) won't be able to translate. The
//! validator runs as a pre-pass — both `eval::run` and the bytecode
//! VM compile path drain its errors and bail before executing any
//! code, so a malformed visual block fails fast at load time.
//!
//! Allowed inside a `visual` body:
//! - Number / bool literals; tuple constructions of those.
//! - Identifier references (parameter names, let-bound locals).
//! - Field access (`uv.x`, `color.red`).
//! - Binary / unary arithmetic and logic ops.
//! - Calls to whitelisted GPU-safe builtins (`smoothstep`, `mix`,
//!   `noise`, the pure `math.*` functions).
//! - `if` / `elif` / `else`, `let`, `return`.
//!
//! Rejected:
//! - String / list / map / range / index / interpolation literals.
//! - Calls to anything not on the allow-list (no `print`, `load`,
//!   `spawn`, `wait`, host I/O).
//! - `while` / `for` (would need bounded-loop unrolling — defer).
//! - Mutation via `=` / `+=` / etc.

use crate::ast::{DeclKind, DeclMember, Expr, Program, Stmt};

/// One subset-violation diagnostic. Position carries through to the
/// caller so they can present a precise span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualError {
    pub message: String,
    pub help: Option<String>,
    pub line: u32,
    pub col: u32,
}

/// Bare names callable inside a visual body. These map 1:1 onto WGSL
/// builtins (or our own WGSL helpers shipping in session 10) with
/// matching semantics.
const ALLOWED_BARE_FNS: &[&str] = &["smoothstep", "mix", "noise"];

/// `math.<name>` calls. Pure number-in-number-out — every WGSL
/// implementation is trivial.
const ALLOWED_MATH_FNS: &[&str] = &[
    "abs",
    "sqrt",
    "floor",
    "ceil",
    "min",
    "max",
    "sin",
    "cos",
    "smoothstep",
    "mix",
    "noise",
];

/// Named color constants on the `color` module. Only the static
/// palette is allowed — `color.from_hex(...)` etc. produce values
/// at runtime today, and folding them into shader constants is its
/// own session of work.
const ALLOWED_COLOR_FIELDS: &[&str] = &[
    "red",
    "green",
    "blue",
    "cyan",
    "yellow",
    "orange",
    "purple",
    "white",
    "black",
    "gray",
    "transparent",
];

/// Run the subset checker over the whole program. Empty Vec means
/// every visual block is GPU-safe.
pub fn check_program(program: &Program) -> Vec<VisualError> {
    let mut errors = Vec::new();
    for stmt in &program.stmts {
        if let Stmt::Decl {
            kind: DeclKind::Visual,
            members,
            ..
        } = stmt
        {
            check_visual_members(members, &mut errors);
        }
    }
    errors
}

fn check_visual_members(members: &[DeclMember], errors: &mut Vec<VisualError>) {
    let mut saw_pixel = false;
    for member in members {
        match member {
            DeclMember::Field { value, .. } => {
                // Field initializers must be GPU-safe constant exprs
                // (size: (64, 96), etc.). The full check applies.
                check_expr(value, errors);
            }
            DeclMember::Method {
                name, params, body, ..
            } => {
                if name == "pixel" {
                    saw_pixel = true;
                    check_pixel_signature(params, body, errors);
                }
                for stmt in body {
                    check_stmt(stmt, errors);
                }
            }
            // States / initial-state aren't valid inside visual blocks
            // (the parser accepts them generically because parse_decl
            // is shared with entity / scene). Surface a clear error.
            DeclMember::InitialState { name, line, col } => errors.push(VisualError {
                message: format!("`initial state {name}` is not allowed inside a `visual` block"),
                help: Some(
                    "visual blocks are pure; states / state machines belong on `entity` blocks"
                        .to_string(),
                ),
                line: *line,
                col: *col,
            }),
            DeclMember::State {
                name, line, col, ..
            } => errors.push(VisualError {
                message: format!("`state {name}` is not allowed inside a `visual` block"),
                help: None,
                line: *line,
                col: *col,
            }),
        }
    }
    if !saw_pixel && !members.is_empty() {
        // Empty visual blocks are tolerated (declarative noop); only
        // flag missing pixel when other members exist, since a
        // parse-bare `visual Foo:` would already have failed indent.
        let (line, col) = first_member_pos(members);
        errors.push(VisualError {
            message: "visual block requires a `pixel(uv, time) -> color:` method".to_string(),
            help: Some(
                "every visual compiles to a fragment shader; `pixel` is the entry point"
                    .to_string(),
            ),
            line,
            col,
        });
    }
}

fn first_member_pos(members: &[DeclMember]) -> (u32, u32) {
    match members.first() {
        Some(DeclMember::Field { line, col, .. }) => (*line, *col),
        Some(DeclMember::Method { line, col, .. }) => (*line, *col),
        _ => (0, 0),
    }
}

fn check_pixel_signature(
    params: &[crate::ast::Param],
    body: &[Stmt],
    errors: &mut Vec<VisualError>,
) {
    // Required signature: `pixel(uv, time) -> color`. We don't
    // enforce the parameter NAMES today (the WGSL compiler will
    // alias them at codegen time), but we do enforce arity so
    // typos like `pixel(uv)` fail at validate time.
    if params.len() != 2 {
        let (line, col) = body
            .first()
            .map(|s| (stmt_line(s), stmt_col(s)))
            .unwrap_or((0, 0));
        errors.push(VisualError {
            message: format!(
                "`pixel` takes exactly two parameters (uv, time), got {}",
                params.len()
            ),
            help: Some(
                "the WGSL compiler binds uv: vec2<f32> and time: f32 — both are required"
                    .to_string(),
            ),
            line,
            col,
        });
    }
}

fn check_stmt(stmt: &Stmt, errors: &mut Vec<VisualError>) {
    match stmt {
        Stmt::Let { value, .. } => check_expr(value, errors),
        Stmt::If {
            cond,
            then_body,
            elifs,
            else_body,
            ..
        } => {
            check_expr(cond, errors);
            for s in then_body {
                check_stmt(s, errors);
            }
            for (c, body) in elifs {
                check_expr(c, errors);
                for s in body {
                    check_stmt(s, errors);
                }
            }
            if let Some(body) = else_body {
                for s in body {
                    check_stmt(s, errors);
                }
            }
        }
        Stmt::Return { value: Some(e), .. } => check_expr(e, errors),
        Stmt::Return { .. } => {}
        Stmt::Expr(e) => check_expr(e, errors),
        // Everything else is rejected. Calling out the worst footguns
        // explicitly so the error message points the user at the
        // right fix.
        Stmt::Assign { line, col, .. } => errors.push(VisualError {
            message: "assignment is not allowed inside a `visual` body".to_string(),
            help: Some(
                "shader code can't mutate previously-bound names; use a fresh `let` instead"
                    .to_string(),
            ),
            line: *line,
            col: *col,
        }),
        Stmt::While { line, col, .. } => errors.push(VisualError {
            message: "`while` loops aren't allowed inside a `visual` body".to_string(),
            help: Some(
                "fragment shaders can't run unbounded loops; rewrite as a closed-form expression"
                    .to_string(),
            ),
            line: *line,
            col: *col,
        }),
        Stmt::For { line, col, .. } => errors.push(VisualError {
            message: "`for` loops aren't allowed inside a `visual` body".to_string(),
            help: Some(
                "fragment shaders can't iterate over runtime ranges; defer to a future bounded-loop session"
                    .to_string(),
            ),
            line: *line,
            col: *col,
        }),
        Stmt::Wait { line, col, .. } => errors.push(VisualError {
            message: "`wait` is not allowed inside a `visual` body".to_string(),
            help: Some("fibers don't run on the GPU".to_string()),
            line: *line,
            col: *col,
        }),
        Stmt::Spawn { line, col, .. } => errors.push(VisualError {
            message: "`spawn` is not allowed inside a `visual` body".to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Stmt::Despawn { line, col, .. } => errors.push(VisualError {
            message: "`despawn` is not allowed inside a `visual` body".to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Stmt::Decl { line, col, .. } => errors.push(VisualError {
            message: "nested declarations are not allowed inside a `visual` body".to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Stmt::FunctionDecl { line, col, .. } => errors.push(VisualError {
            message: "function declarations are not allowed inside a `visual` body"
                .to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Stmt::Break { line, col, .. } | Stmt::Continue { line, col, .. } => {
            errors.push(VisualError {
                message: "loop control statements aren't allowed (no loops in shaders)"
                    .to_string(),
                help: None,
                line: *line,
                col: *col,
            })
        }
        Stmt::Transition { line, col, .. } => errors.push(VisualError {
            message: "state transitions aren't allowed inside a `visual` body".to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Stmt::OnUpdate { line, col, .. }
        | Stmt::OnRender { line, col, .. }
        | Stmt::OnClassEvent { line, col, .. } => errors.push(VisualError {
            message: "event handlers aren't allowed inside a `visual` body".to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Stmt::DialogueDecl { line, col, .. }
        | Stmt::Say { line, col, .. }
        | Stmt::Choice { line, col, .. } => errors.push(VisualError {
            message: "dialogue constructs aren't allowed inside a `visual` body"
                .to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Stmt::Import { line, col, .. } => errors.push(VisualError {
            message: "`import` statements aren't allowed inside a `visual` body"
                .to_string(),
            help: Some(
                "move the `import` to the top of the script and reference the module from there"
                    .to_string(),
            ),
            line: *line,
            col: *col,
        }),
    }
}

fn check_expr(expr: &Expr, errors: &mut Vec<VisualError>) {
    match expr {
        Expr::Int { .. }
        | Expr::Float { .. }
        | Expr::Bool { .. }
        | Expr::Percent { .. }
        | Expr::Quantity { .. }
        | Expr::Ident { .. }
        | Expr::SelfRef { .. } => {}
        Expr::Tuple { elems, .. } => {
            for e in elems {
                check_expr(e, errors);
            }
        }
        Expr::Binary { left, right, .. } => {
            check_expr(left, errors);
            check_expr(right, errors);
        }
        Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            ..
        } => {
            // WGSL has both `select(a, b, cond)` and ternary conditional
            // expressions, so the if-expression form is GPU-safe — recurse
            // into every branch to keep the existing per-construct checks
            // (no allocation, no mutation, etc.) wired through.
            check_expr(cond, errors);
            check_expr(then_expr, errors);
            for (c, e) in elifs {
                check_expr(c, errors);
                check_expr(e, errors);
            }
            check_expr(else_expr, errors);
        }
        Expr::Unary { operand, .. } => check_expr(operand, errors),
        Expr::Field { object, .. } => {
            // The field-access form covers `uv.x`, `color.red`, and
            // also the receiver chain in `math.sin(...)`. We don't
            // restrict field READS — the call-site check below
            // handles the function-call case.
            check_expr(object, errors);
        }
        Expr::Call { callee, args, .. } => {
            check_callable(callee, errors);
            for a in args {
                check_expr(a, errors);
            }
            // Kwargs get the same treatment as positional args.
        }
        // Allocating / non-GPU forms.
        Expr::Str { line, col, .. } => errors.push(VisualError {
            message: "string literals aren't allowed inside a `visual` body".to_string(),
            help: Some(
                "shaders work on numeric data; build colors from tuples or `color.<name>`"
                    .to_string(),
            ),
            line: *line,
            col: *col,
        }),
        Expr::Interp { line, col, .. } => errors.push(VisualError {
            message: "string interpolation isn't allowed inside a `visual` body".to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Expr::List { line, col, .. } => errors.push(VisualError {
            message: "list literals aren't allowed inside a `visual` body".to_string(),
            help: Some(
                "shaders can't allocate; use a tuple `(a, b, c)` if you mean a fixed-shape vector"
                    .to_string(),
            ),
            line: *line,
            col: *col,
        }),
        Expr::Range { line, col, .. } => errors.push(VisualError {
            message: "range literals aren't allowed inside a `visual` body".to_string(),
            help: None,
            line: *line,
            col: *col,
        }),
        Expr::Index { line, col, .. } => errors.push(VisualError {
            message: "indexing isn't allowed inside a `visual` body".to_string(),
            help: Some("use `.x` / `.y` / `.z` / `.w` swizzles on tuples".to_string()),
            line: *line,
            col: *col,
        }),
        // Phase 33 session 9: typed holes are an authoring affordance
        // for top-level Twe code; they don't lower to WGSL because
        // there's no shader-side runtime panic. Reject early with a
        // clear message.
        Expr::Hole { line, col } => errors.push(VisualError {
            message: "typed holes `???` aren't allowed inside a `visual` body".to_string(),
            help: Some(
                "fill in the expression before compiling the shader; visual blocks lower to WGSL and must be complete".to_string(),
            ),
            line: *line,
            col: *col,
        }),
    }
}

/// Validate that a `Call.callee` resolves to a GPU-safe function.
/// Accepts:
///   - bare ident on the allow-list (`smoothstep`, `mix`, `noise`)
///   - `math.<name>` where `<name>` is on the math allow-list
///   - `color.<name>` is field access (not a call), but we surface
///     a helpful error if the user tries to call it like a function
fn check_callable(callee: &Expr, errors: &mut Vec<VisualError>) {
    match callee {
        Expr::Ident { name, line, col } => {
            if !ALLOWED_BARE_FNS.contains(&name.as_str()) {
                errors.push(VisualError {
                    message: format!("function `{name}` is not allowed inside a `visual` body"),
                    help: Some(format!(
                        "GPU-safe bare names: {}",
                        ALLOWED_BARE_FNS.join(", ")
                    )),
                    line: *line,
                    col: *col,
                });
            }
        }
        Expr::Field {
            object,
            name,
            line,
            col,
        } => match object.as_ref() {
            Expr::Ident { name: module, .. } if module == "math" => {
                if !ALLOWED_MATH_FNS.contains(&name.as_str()) {
                    errors.push(VisualError {
                        message: format!(
                            "function `math.{name}` is not allowed inside a `visual` body"
                        ),
                        help: Some(format!(
                            "GPU-safe `math.*` calls: {}",
                            ALLOWED_MATH_FNS.join(", ")
                        )),
                        line: *line,
                        col: *col,
                    });
                }
            }
            Expr::Ident { name: module, .. } if module == "color" => {
                if !ALLOWED_COLOR_FIELDS.contains(&name.as_str()) {
                    errors.push(VisualError {
                        message: format!(
                            "`color.{name}` is not callable inside a `visual` body"
                        ),
                        help: Some(format!(
                            "GPU-safe `color.*` reads: {} (constructors like `from_hex` defer to runtime)",
                            ALLOWED_COLOR_FIELDS.join(", ")
                        )),
                        line: *line,
                        col: *col,
                    });
                } else {
                    // Calling color.red() (rather than reading it as a value)
                    // is a semantic error too — the named constants aren't
                    // functions.
                    errors.push(VisualError {
                        message: format!(
                            "`color.{name}` is a constant, not a function — drop the parentheses"
                        ),
                        help: None,
                        line: *line,
                        col: *col,
                    });
                }
            }
            _ => {
                errors.push(VisualError {
                    message:
                        "method calls on arbitrary receivers aren't allowed inside a `visual` body"
                            .to_string(),
                    help: Some(
                        "GPU-safe calls are bare `smoothstep` / `mix` / `noise` or `math.<name>`"
                            .to_string(),
                    ),
                    line: *line,
                    col: *col,
                });
            }
        },
        _ => {
            errors.push(VisualError {
                message: "computed callees aren't allowed inside a `visual` body".to_string(),
                help: None,
                line: 0,
                col: 0,
            });
        }
    }
}

fn stmt_line(stmt: &Stmt) -> u32 {
    match stmt {
        Stmt::Let { line, .. }
        | Stmt::Assign { line, .. }
        | Stmt::If { line, .. }
        | Stmt::Return { line, .. }
        | Stmt::While { line, .. }
        | Stmt::For { line, .. }
        | Stmt::Break { line, .. }
        | Stmt::Continue { line, .. }
        | Stmt::Transition { line, .. }
        | Stmt::Spawn { line, .. }
        | Stmt::Despawn { line, .. }
        | Stmt::Wait { line, .. }
        | Stmt::DialogueDecl { line, .. }
        | Stmt::Say { line, .. }
        | Stmt::Choice { line, .. }
        | Stmt::OnUpdate { line, .. }
        | Stmt::OnRender { line, .. }
        | Stmt::OnClassEvent { line, .. }
        | Stmt::Decl { line, .. }
        | Stmt::FunctionDecl { line, .. }
        | Stmt::Import { line, .. } => *line,
        Stmt::Expr(e) => e.line(),
    }
}

fn stmt_col(stmt: &Stmt) -> u32 {
    match stmt {
        Stmt::Let { col, .. }
        | Stmt::Assign { col, .. }
        | Stmt::If { col, .. }
        | Stmt::Return { col, .. }
        | Stmt::While { col, .. }
        | Stmt::For { col, .. }
        | Stmt::Break { col, .. }
        | Stmt::Continue { col, .. }
        | Stmt::Transition { col, .. }
        | Stmt::Spawn { col, .. }
        | Stmt::Despawn { col, .. }
        | Stmt::Wait { col, .. }
        | Stmt::DialogueDecl { col, .. }
        | Stmt::Say { col, .. }
        | Stmt::Choice { col, .. }
        | Stmt::OnUpdate { col, .. }
        | Stmt::OnRender { col, .. }
        | Stmt::OnClassEvent { col, .. }
        | Stmt::Decl { col, .. }
        | Stmt::FunctionDecl { col, .. }
        | Stmt::Import { col, .. } => *col,
        Stmt::Expr(e) => e.col(),
    }
}
