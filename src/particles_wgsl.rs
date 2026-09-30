//! web3d-M7: compile a `particles` block's `on_spawn(p)` / `on_update(p, dt)`
//! to WGSL, so the 3D runtime simulates its particles on the GPU
//! (`kernel::particles`; design note
//! `docs/changes/2026-09-30-web3d-m7-gpu-particles.md`).
//!
//! The subset is what a particle body needs: `let` locals, `if`,
//! assignments (plain and compound) to the particle's `pos`,
//! `velocity`, `color` and `size` or to locals, arithmetic, tuples as
//! vectors, `vec3(...)`, the `math.*` functions a shader has,
//! `random.float()`, `color.<name>`, and the emitter's own fields when
//! their defaults are literals. Anything else (a global, `print`, a
//! loop, a `render()` override, setting `age` or `lifetime`) returns
//! an error, and the block keeps running on the CPU — the same program,
//! just slower. Numbers are `f32` on the GPU.
//!
//! The compiler type-checks what it emits (numbers, 2–4 component
//! vectors, booleans), so its WGSL is valid by construction and the web
//! build carries no WGSL validator; tests check the output with naga.
//! It accepts only what the CPU also accepts (no component assignment,
//! no tuple × tuple), and it tracks which numbers the CPU may hold as
//! integers, refusing a division that would truncate there: the same
//! block must mean the same thing on both.

use crate::ast::{AssignOp, AssignTarget, BinOp, DeclMember, Expr, Stmt, UnOp};
use crate::kernel::particles::ParticleProgram;
use std::collections::HashMap;
use std::fmt::Write;

/// A value's type: a number (`f32`), an N-vector (`vecN<f32>`, N =
/// 2..4, a tuple on the CPU) or a boolean. `float` says the CPU
/// certainly holds floats here; otherwise they may be integers (an int
/// literal, `math.floor`), and dividing two integers truncates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    N { float: bool },
    V { n: u8, float: bool },
    B,
}

const FLOAT: Ty = Ty::N { float: true };
const MAYBE_INT: Ty = Ty::N { float: false };

impl Ty {
    fn name(self) -> String {
        match self {
            Ty::N { .. } => "a number".to_string(),
            Ty::V { n, .. } => format!("a {n}-vector"),
            Ty::B => "a boolean".to_string(),
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Ty::N { float: true } | Ty::V { float: true, .. })
    }

    fn with_float(self, float: bool) -> Ty {
        match self {
            Ty::N { .. } => Ty::N { float },
            Ty::V { n, .. } => Ty::V { n, float },
            Ty::B => Ty::B,
        }
    }

    fn same_shape(self, other: Ty) -> bool {
        self.with_float(true) == other.with_float(true)
    }

    fn is_number(self) -> bool {
        matches!(self, Ty::N { .. })
    }

    fn is_vector(self) -> bool {
        matches!(self, Ty::V { .. })
    }
}

/// The particle's fields, as the GPU `Particle` struct has them. The
/// runtime keeps `age`, `age_ratio` and `lifetime` floats; the others
/// hold whatever a body (or `spawn ... at`) put there.
fn field_ty(name: &str) -> Option<Ty> {
    Some(match name {
        "pos" | "velocity" => Ty::V { n: 3, float: false },
        "color" => Ty::V { n: 4, float: false },
        "size" => MAYBE_INT,
        "age" | "age_ratio" | "lifetime" => FLOAT,
        _ => return None,
    })
}

/// The fields a body may set. `age` / `lifetime` decide when the
/// emitter despawns, which the simulation must know exactly, so a
/// body setting them runs on the CPU.
const WRITABLE: &[&str] = &["pos", "velocity", "color", "size"];

/// Compile particles block `name`'s methods to a [`ParticleProgram`],
/// or say why it can't run on the GPU.
pub fn compile(name: &str, members: &[DeclMember]) -> Result<ParticleProgram, String> {
    let mut fields: HashMap<&str, &Expr> = HashMap::new();
    let mut spawn = None;
    let mut update = None;
    for m in members {
        match m {
            DeclMember::Field { name, value, .. } => {
                fields.insert(name.as_str(), value);
            }
            DeclMember::Method { name: n, params, body, .. } => match n.as_str() {
                "on_spawn" => spawn = Some((params, body)),
                "on_update" => update = Some((params, body)),
                "render" => {
                    return Err(format!("particles `{name}` draws itself with render()"));
                }
                _ => {}
            },
            _ => {}
        }
    }
    let collide = match fields.get("collide") {
        None => false,
        Some(Expr::Bool { value, .. }) => *value,
        Some(_) => return Err("`collide` must be true or false".to_string()),
    };
    let mut out = ParticleProgram {
        spawn: String::new(),
        update: String::new(),
        collide,
    };
    if let Some((params, body)) = spawn {
        if params.len() != 1 {
            return Err("on_spawn takes one parameter, the particle".to_string());
        }
        let mut cx = Codegen::new(&fields, &params[0].name, None);
        cx.block(&mut out.spawn, body)?;
    }
    if let Some((params, body)) = update {
        if params.len() != 2 {
            return Err("on_update takes two parameters, the particle and dt".to_string());
        }
        let mut cx = Codegen::new(&fields, &params[0].name, Some(&params[1].name));
        cx.block(&mut out.update, body)?;
    }
    Ok(out)
}

struct Codegen<'a> {
    fields: &'a HashMap<&'a str, &'a Expr>,
    particle: &'a str,
    dt: Option<&'a str>,
    /// Locals in scope with their types, innermost block last.
    scopes: Vec<Vec<(String, Ty)>>,
    indent: usize,
}

fn unsupported(what: &str, line: u32) -> String {
    format!("line {line}: {what} can't run on the GPU")
}

fn mismatch(what: &str, tys: &[Ty], line: u32) -> String {
    let tys: Vec<String> = tys.iter().map(|t| t.name()).collect();
    format!("line {line}: {what} doesn't take {}", tys.join(" and "))
}

/// The type of `a <op> b` for arithmetic, where the CPU allows it:
/// numbers; vectors of one size added or subtracted; a vector
/// multiplied by a number (either side) or divided by one. A division
/// the CPU might do on integers is refused.
fn arith_ty(op: BinOp, a: Ty, b: Ty, line: u32) -> Result<Ty, String> {
    let float = a.is_float() || b.is_float();
    let shape = match (a, b) {
        (Ty::N { .. }, Ty::N { .. }) => Some(a),
        (Ty::V { n, .. }, Ty::V { n: m, .. }) if n == m && matches!(op, BinOp::Add | BinOp::Sub) => Some(a),
        (Ty::V { .. }, Ty::N { .. }) if matches!(op, BinOp::Mul | BinOp::Div) => Some(a),
        (Ty::N { .. }, Ty::V { .. }) if op == BinOp::Mul => Some(b),
        _ => None,
    };
    let text = match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        _ => "/",
    };
    let shape = shape.ok_or_else(|| mismatch(&format!("`{text}`"), &[a, b], line))?;
    if op == BinOp::Div && !float {
        return Err(format!(
            "line {line}: this division may divide integers, which truncates on the CPU but not on the GPU; write the divisor as a float (e.g. `2.0`)"
        ));
    }
    Ok(shape.with_float(float))
}

/// The type of component `name` of a value of type `ty`.
fn component(ty: Ty, name: &str, line: u32) -> Result<Ty, String> {
    let index = match name {
        "x" | "r" => 0,
        "y" | "g" => 1,
        "z" | "b" => 2,
        "w" | "a" => 3,
        _ => return Err(unsupported(&format!("`.{name}`"), line)),
    };
    match ty {
        Ty::V { n, float } if index < n => Ok(Ty::N { float }),
        _ => Err(format!("line {line}: {} has no `.{name}`", ty.name())),
    }
}

impl<'a> Codegen<'a> {
    fn new(fields: &'a HashMap<&'a str, &'a Expr>, particle: &'a str, dt: Option<&'a str>) -> Self {
        Codegen {
            fields,
            particle,
            dt,
            scopes: vec![Vec::new()],
            indent: 1,
        }
    }

    fn local(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .find(|(n, _)| n == name)
            .map(|(_, t)| *t)
    }

    /// Record that local `name` may now hold `ty`'s kind of number.
    fn assigned(&mut self, name: &str, ty: Ty) {
        if let Some(slot) = self
            .scopes
            .iter_mut()
            .rev()
            .flat_map(|s| s.iter_mut().rev())
            .find(|(n, _)| n == name)
        {
            slot.1 = slot.1.with_float(slot.1.is_float() && ty.is_float());
        }
    }

    fn block(&mut self, out: &mut String, body: &[Stmt]) -> Result<(), String> {
        self.scopes.push(Vec::new());
        for s in body {
            self.stmt(out, s)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn pad(&self, out: &mut String) {
        for _ in 0..self.indent {
            out.push_str("    ");
        }
    }

    fn stmt(&mut self, out: &mut String, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let { name, value, line, .. } => {
                if self.scopes.last().is_some_and(|s| s.iter().any(|(n, _)| n == name)) {
                    return Err(unsupported(&format!("declaring `{name}` twice"), *line));
                }
                self.pad(out);
                write!(out, "var v_{name} = ").unwrap();
                let ty = self.expr(out, value)?;
                out.push_str(";\n");
                self.scopes.last_mut().expect("a scope").push((name.clone(), ty));
            }
            Stmt::Assign {
                target, op, value, line, ..
            } => {
                self.pad(out);
                let target_ty = match target {
                    AssignTarget::Name(name, _) => match self.local(name) {
                        Some(t) => {
                            write!(out, "v_{name}").unwrap();
                            t
                        }
                        None => {
                            return Err(unsupported(&format!("setting `{name}` (not a local)"), *line));
                        }
                    },
                    AssignTarget::Field { object, name } => self.place(out, object, name, *line)?,
                };
                let (text, arith) = match op {
                    AssignOp::Set => (" = ", None),
                    AssignOp::AddAssign => (" += ", Some(BinOp::Add)),
                    AssignOp::SubAssign => (" -= ", Some(BinOp::Sub)),
                    AssignOp::MulAssign => (" *= ", Some(BinOp::Mul)),
                    AssignOp::DivAssign => (" /= ", Some(BinOp::Div)),
                };
                out.push_str(text);
                let value_ty = self.expr(out, value)?;
                let result = match arith {
                    None => value_ty,
                    Some(op) => arith_ty(op, target_ty, value_ty, *line)?,
                };
                if !result.same_shape(target_ty) {
                    return Err(format!(
                        "line {line}: can't assign {} to {}",
                        value_ty.name(),
                        target_ty.name()
                    ));
                }
                if let AssignTarget::Name(name, _) = target {
                    self.assigned(name, result);
                }
                out.push_str(";\n");
            }
            Stmt::If {
                cond,
                then_body,
                elifs,
                else_body,
                ..
            } => {
                self.pad(out);
                out.push_str("if (");
                self.condition(out, cond)?;
                out.push_str(") {\n");
                self.nested(out, then_body)?;
                for (c, body) in elifs {
                    out.push_str(" else if (");
                    self.condition(out, c)?;
                    out.push_str(") {\n");
                    self.nested(out, body)?;
                }
                if let Some(body) = else_body {
                    out.push_str(" else {\n");
                    self.nested(out, body)?;
                }
                out.push('\n');
            }
            Stmt::Return { value: None, .. } => {
                self.pad(out);
                out.push_str("return pt;\n");
            }
            Stmt::Expr(e) => {
                // Name what can't run (`print(...)`) when it's the call.
                self.expr(&mut String::new(), e)?;
                return Err(unsupported("an expression statement", e.line()));
            }
            other => return Err(unsupported("this statement", other.line())),
        }
        Ok(())
    }

    /// A block's statements then its closing brace (no newline).
    fn nested(&mut self, out: &mut String, body: &[Stmt]) -> Result<(), String> {
        self.indent += 1;
        self.block(out, body)?;
        self.indent -= 1;
        self.pad(out);
        out.push('}');
        Ok(())
    }

    /// An `if` condition: must be a boolean.
    fn condition(&self, out: &mut String, cond: &Expr) -> Result<(), String> {
        match self.expr(out, cond)? {
            Ty::B => Ok(()),
            t => Err(format!("line {}: a condition must be a boolean, not {}", cond.line(), t.name())),
        }
    }

    /// An assignable field: `p.<writable>`. (The CPU can't set one
    /// component of a tuple, so neither can a GPU body.) Returns its type.
    fn place(&self, out: &mut String, object: &Expr, name: &str, line: u32) -> Result<Ty, String> {
        match object {
            Expr::Ident { name: o, .. } if o == self.particle => {
                if !WRITABLE.contains(&name) {
                    return Err(unsupported(&format!("setting the particle's `{name}`"), line));
                }
                write!(out, "pt.{name}").unwrap();
                Ok(field_ty(name).expect("writable fields exist"))
            }
            _ => Err(unsupported(&format!("setting `.{name}` here"), line)),
        }
    }

    /// Emit `expr`, returning its type.
    fn expr(&self, out: &mut String, expr: &Expr) -> Result<Ty, String> {
        Ok(match expr {
            Expr::Int { value, .. } => {
                write!(out, "{value}.0").unwrap();
                MAYBE_INT
            }
            Expr::Float { value, .. } => {
                write!(out, "{value:?}").unwrap();
                FLOAT
            }
            Expr::Bool { value, .. } => {
                write!(out, "{value}").unwrap();
                Ty::B
            }
            Expr::Ident { name, line, .. } => {
                if name == self.particle {
                    return Err(unsupported("the particle as a value", *line));
                } else if Some(name.as_str()) == self.dt {
                    out.push_str("dt");
                    FLOAT
                } else if let Some(t) = self.local(name) {
                    write!(out, "v_{name}").unwrap();
                    t
                } else {
                    return self.emitter_field(out, name, *line);
                }
            }
            Expr::Tuple { elems, line, .. } => self.vector(out, "a tuple", elems, *line)?,
            Expr::Binary { op, left, right, line, .. } => {
                let (text, kind) = match op {
                    BinOp::Add => ("+", 0),
                    BinOp::Sub => ("-", 0),
                    BinOp::Mul => ("*", 0),
                    BinOp::Div => ("/", 0),
                    BinOp::Eq => ("==", 1),
                    BinOp::Neq => ("!=", 1),
                    BinOp::Lt => ("<", 1),
                    BinOp::Lte => ("<=", 1),
                    BinOp::Gt => (">", 1),
                    BinOp::Gte => (">=", 1),
                    BinOp::And => ("&&", 2),
                    BinOp::Or => ("||", 2),
                    BinOp::In | BinOp::NotIn => return Err(unsupported("`in`", *line)),
                };
                out.push('(');
                let a = self.expr(out, left)?;
                write!(out, " {text} ").unwrap();
                let b = self.expr(out, right)?;
                out.push(')');
                let ty = match kind {
                    0 => return arith_ty(*op, a, b, *line),
                    1 => (a.is_number() && b.is_number()).then_some(Ty::B),
                    _ => (a == Ty::B && b == Ty::B).then_some(Ty::B),
                };
                ty.ok_or_else(|| mismatch(&format!("`{text}`"), &[a, b], *line))?
            }
            Expr::Unary { op, operand, line, .. } => {
                out.push_str(match op {
                    UnOp::Neg => "-(",
                    UnOp::Not => "!(",
                });
                let t = self.expr(out, operand)?;
                out.push(')');
                match (op, t) {
                    (UnOp::Neg, Ty::N { .. }) | (UnOp::Not, Ty::B) => t,
                    _ => return Err(mismatch("this operator", &[t], *line)),
                }
            }
            Expr::IfExpr {
                cond,
                then_expr,
                elifs,
                else_expr,
                line,
                ..
            } => {
                if !elifs.is_empty() {
                    return Err(unsupported("`elif` in an if-expression", *line));
                }
                out.push_str("select(");
                let b = self.expr(out, else_expr)?;
                out.push_str(", ");
                let a = self.expr(out, then_expr)?;
                out.push_str(", ");
                self.condition(out, cond)?;
                out.push(')');
                if !a.same_shape(b) {
                    return Err(mismatch("an if-expression's branches", &[a, b], *line));
                }
                a.with_float(a.is_float() && b.is_float())
            }
            Expr::Field { object, name, line, .. } => match object.as_ref() {
                Expr::Ident { name: o, .. } if o == self.particle => {
                    let ty = field_ty(name).ok_or_else(|| format!("line {line}: particles have no field `{name}`"))?;
                    write!(out, "pt.{name}").unwrap();
                    ty
                }
                Expr::Ident { name: o, .. } if o == "color" && self.local(o).is_none() => {
                    let c = crate::visual_wgsl::color_constant(name)
                        .ok_or_else(|| format!("line {line}: unknown `color.{name}`"))?;
                    write!(out, "vec4<f32>({:?}, {:?}, {:?}, {:?})", c[0], c[1], c[2], c[3]).unwrap();
                    Ty::V { n: 4, float: true }
                }
                Expr::Ident { name: o, .. } if o == "math" && name == "pi" && self.local(o).is_none() => {
                    out.push_str("3.14159265");
                    FLOAT
                }
                Expr::SelfRef { .. } => self.emitter_field(out, name, *line)?,
                _ => {
                    out.push('(');
                    let t = self.expr(out, object)?;
                    write!(out, ").{name}").unwrap();
                    component(t, name, *line)?
                }
            },
            Expr::Call {
                callee, args, kwargs, line, ..
            } => {
                if !kwargs.is_empty() {
                    return Err(unsupported("named arguments", *line));
                }
                let name = match callee.as_ref() {
                    Expr::Ident { name, .. } => name.clone(),
                    Expr::Field { object, name, .. } => match object.as_ref() {
                        Expr::Ident { name: m, .. } => format!("{m}.{name}"),
                        _ => return Err(unsupported("this call", *line)),
                    },
                    _ => return Err(unsupported("this call", *line)),
                };
                self.call(out, &name, args, *line)?
            }
            other => return Err(unsupported("this expression", other.line())),
        })
    }

    /// `vecN<f32>(...)` from N numbers.
    fn vector(&self, out: &mut String, what: &str, elems: &[Expr], line: u32) -> Result<Ty, String> {
        if !(2..=4).contains(&elems.len()) {
            return Err(unsupported(&format!("{what} that isn't 2, 3 or 4 numbers"), line));
        }
        write!(out, "vec{}<f32>", elems.len()).unwrap();
        let tys = self.args(out, elems)?;
        if !tys.iter().all(|t| t.is_number()) {
            return Err(mismatch(what, &tys, line));
        }
        Ok(Ty::V {
            n: elems.len() as u8,
            float: tys.iter().all(|t| t.is_float()),
        })
    }

    /// A call to a builtin the GPU has, checked against its signature.
    fn call(&self, out: &mut String, name: &str, args: &[Expr], line: u32) -> Result<Ty, String> {
        let base = name.strip_prefix("math.").unwrap_or(name);
        match name {
            "random.float" if args.is_empty() => {
                out.push_str("twe_rand()");
                return Ok(FLOAT);
            }
            // `vec3` converts its arguments to floats.
            "vec3" if args.len() == 3 => return Ok(self.vector(out, "`vec3`", args, line)?.with_float(true)),
            "smoothstep" | "mix" | "noise" => {}
            _ if name.starts_with("math.") => {}
            _ => return Err(unsupported(&format!("`{name}(...)`"), line)),
        }
        let wgsl = match base {
            "atan2" => "atan2",
            "mod" => "twe_mod",
            "abs" | "sqrt" | "floor" | "ceil" | "sin" | "cos" | "min" | "max" | "clamp" | "smoothstep" | "mix"
            | "noise" | "dot" | "cross" | "length" | "normalize" => base,
            _ => return Err(unsupported(&format!("`{name}(...)`"), line)),
        };
        out.push_str(wgsl);
        let tys = self.args(out, args)?;
        // Which arguments the CPU's builtin takes, and what it returns.
        let numbers = |k: usize| tys.len() == k && tys.iter().all(|t| t.is_number());
        let all_float = tys.iter().all(|t| t.is_float());
        let vectors = |k: usize| tys.len() == k && tys.iter().all(|t| t.is_vector() && t.same_shape(tys[0]));
        let result = match base {
            "abs" if numbers(1) => Some(tys[0]),
            "min" | "max" if numbers(2) => Some(Ty::N { float: all_float }),
            "clamp" if numbers(3) => Some(Ty::N { float: all_float }),
            "mod" if numbers(2) => Some(Ty::N { float: all_float }),
            "floor" | "ceil" if numbers(1) => Some(MAYBE_INT),
            "sqrt" | "sin" | "cos" if numbers(1) => Some(FLOAT),
            "atan2" if numbers(2) => Some(FLOAT),
            "smoothstep" if numbers(3) => Some(FLOAT),
            "mix" if numbers(3) => Some(FLOAT),
            "mix" if tys.len() == 3 && tys[0].is_vector() && tys[0].same_shape(tys[1]) && tys[2].is_number() => {
                Some(tys[0].with_float(true))
            }
            "noise" if tys.len() == 1 && tys[0].same_shape(Ty::V { n: 2, float: true }) => Some(FLOAT),
            "dot" if vectors(2) => Some(Ty::N {
                float: tys[0].is_float() || tys[1].is_float(),
            }),
            "cross" if vectors(2) && tys[0].same_shape(Ty::V { n: 3, float: true }) => {
                Some(tys[0].with_float(tys[0].is_float() || tys[1].is_float()))
            }
            "length" if vectors(1) => Some(FLOAT),
            "normalize" if vectors(1) => Some(tys[0].with_float(true)),
            _ => None,
        };
        result.ok_or_else(|| mismatch(&format!("`{name}`"), &tys, line))
    }

    fn args(&self, out: &mut String, args: &[Expr]) -> Result<Vec<Ty>, String> {
        out.push('(');
        let mut tys = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            tys.push(self.expr(out, a)?);
        }
        out.push(')');
        Ok(tys)
    }

    /// An emitter field, inlined: only literal defaults are known on
    /// the GPU.
    fn emitter_field(&self, out: &mut String, name: &str, line: u32) -> Result<Ty, String> {
        match self.fields.get(name) {
            Some(value) if is_literal(value) => self.expr(out, value),
            Some(_) => Err(unsupported(&format!("emitter field `{name}` (not a literal)"), line)),
            None => Err(unsupported(&format!("`{name}` (not a local or emitter field)"), line)),
        }
    }
}

fn is_literal(e: &Expr) -> bool {
    match e {
        Expr::Int { .. } | Expr::Float { .. } | Expr::Bool { .. } => true,
        Expr::Unary { operand, .. } => is_literal(operand),
        Expr::Tuple { elems, .. } => elems.iter().all(is_literal),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(src: &str) -> Vec<DeclMember> {
        let tokens = crate::lexer::lex(src).expect("lex");
        let program = crate::parser::parse(&tokens).expect("parse");
        match program.stmts.into_iter().next() {
            Some(Stmt::Decl { members, .. }) => members,
            _ => panic!("no decl"),
        }
    }

    const SPARKS: &str = "particles Sparks:
    count: 100
    lifetime: 1.5
    speed: 4.0
    tint: (1.0, 0.8, 0.2, 1.0)
    collide: true

    on_spawn(p):
        let a = random.float() * 2.0 * math.pi
        p.velocity = (math.cos(a) * speed, 3.0, math.sin(a) * speed)
        p.color = color.orange
        p.size = math.max(0.02, random.float() * 0.05)
        if p.size > 0.04 and not (p.size > 0.045):
            p.color = tint

    on_update(p, dt):
        p.velocity -= (0, 9.8, 0) * dt
        p.pos += p.velocity * dt
        let fade = if p.age_ratio > 0.5: 1.0 - p.age_ratio else: 0.5
        var drift = (noise((p.pos.x, p.pos.z)), 0.0)
        drift = drift + (0.0, math.mod(p.age, 0.5))
        p.velocity += (drift.x, 0.0, drift.y) * dt
        p.color = (1.0, 0.3, 0.1, fade * math.clamp(math.length(p.velocity) / 10.0, 0.0, 1.0))
        if p.pos.y < -5.0:
            return
        p.size = math.mix(p.size, 0.01, dt) / 2
";

    #[test]
    fn compiles_a_spark_fountain_that_validates() {
        let program = compile("Sparks", &members(SPARKS)).expect("compiles");
        assert!(program.collide);
        assert!(program.spawn.contains("twe_rand()"), "{}", program.spawn);
        assert!(program.update.contains("pt.pos += (pt.velocity * dt);"), "{}", program.update);
        crate::kernel::particles::tests::validate(&program).expect("valid WGSL");
    }

    #[test]
    fn what_the_gpu_cannot_run_stays_on_the_cpu() {
        let cases = [
            ("    on_spawn(p):\n        print(1)\n", "print"),
            ("    on_spawn(p):\n        p.lifetime = 3.0\n", "lifetime"),
            ("    on_update(p, dt):\n        p.size = score\n", "score"),
            ("    on_spawn(p):\n        p.size = 7 / 2\n", "may divide integers"),
            ("    on_spawn(p):\n        p.size = math.floor(p.age * 4.0) / 2\n", "may divide integers"),
            ("    on_spawn(p):\n        p.pos = p.pos / 2\n", "may divide integers"),
            // What the CPU can't do either.
            ("    on_spawn(p):\n        p.color.a = 0.5\n", "setting `.a`"),
            ("    on_spawn(p):\n        p.pos = -p.pos\n", "this operator"),
            ("    on_spawn(p):\n        p.pos = p.pos * p.pos\n", "`*` doesn't take"),
            ("    on_spawn(p):\n        p.pos = math.abs(p.pos)\n", "`math.abs` doesn't take"),
            ("    function render():\n        print(1)\n", "render"),
            // Type errors: caught here, not by a shader compiler.
            ("    on_spawn(p):\n        p.size = (1.0, 2.0)\n", "can't assign a 2-vector to a number"),
            ("    on_spawn(p):\n        p.pos = p.pos + 1.0\n", "`+` doesn't take a 3-vector and a number"),
            ("    on_spawn(p):\n        if p.size:\n            p.size = 1.0\n", "must be a boolean"),
            ("    on_spawn(p):\n        p.size = p.pos.w\n", "has no `.w`"),
            ("    on_spawn(p):\n        p.size = math.cross(p.pos, p.pos)\n", "can't assign a 3-vector"),
        ];
        for (body, why) in cases {
            let src = format!("particles B:\n    count: 3\n\n{body}");
            let err = compile("B", &members(&src)).expect_err(body);
            assert!(err.contains(why), "{body}: {err}");
        }
    }
}
