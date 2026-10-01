//! web3d-M5 session 2: behavioural grading for the LLM benchmark.
//!
//! A task is a directory under `bench/tasks/`:
//!
//! ```text
//! bench/tasks/<id>/
//!   task.md        the task, in words (shared with the Python baseline)
//!   twe.md         what a Twe program must name for the checks to find
//!   task.toml      ticks, the input script, the checks
//!   solution.twe   a reference solution (must pass)
//!   starter.twe    optional starting file (must fail)
//!   mutants/*.twe  optional hand-written broken variants
//! ```
//!
//! A program is graded by running it headless for `ticks` fixed 60 Hz
//! ticks through the 3D runtime's input path (`host3d::apply_command`,
//! `eval::tick_frame`, `eval::render_frame3d`, as the shells and the
//! soak harness do), feeding it the task's input script, and
//! evaluating each check, a Twe expression that must be `true`, after
//! its tick.
//!
//! `task.toml`:
//!
//! ```toml
//! ticks = 240
//! [[input]]            # keys held for ticks 0..60 (end exclusive)
//! from = 0
//! to = 60
//! hold = ["d"]
//! [[input]]            # a press on tick 90 (pressed keys are also held that tick)
//! at = 90
//! press = ["space"]
//! [[check]]
//! name = "moved right"
//! at = 60              # after 60 ticks; default: after the last
//! expr = "player.pos.x > 2"
//! ```
//!
//! Mouse input: `mouse = [x, y]` (position over the span), `hold_mouse`
//! and `click` (button names, like `hold` / `press`).
//!
//! Grading runs in a child process (`twec bench grade --json`) with a
//! wall-clock limit, so a program that loops forever or crashes the
//! interpreter fails its task instead of stopping the benchmark, and no
//! thread-local engine state carries from one program to the next.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};

use crate::eval;
use crate::replay::InputCommand;
use crate::value::Env;

/// Stdout kept per graded program (the rest is cut).
const STDOUT_CAP: usize = 16 * 1024;

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub dir: PathBuf,
    /// Difficulty, 1 to 3: logic and timing; entities and state;
    /// input-driven play.
    pub tier: u8,
    pub ticks: u32,
    pub inputs: Vec<InputSpan>,
    pub checks: Vec<Check>,
}

/// Input over ticks `from..to`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputSpan {
    pub from: u32,
    pub to: u32,
    pub hold: Vec<String>,
    pub press: Vec<String>,
    pub mouse: Option<(f64, f64)>,
    pub hold_mouse: Vec<String>,
    pub click: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub name: String,
    /// Evaluated after this many ticks.
    pub at: u32,
    pub expr: String,
}

pub fn load_task(dir: &Path) -> Result<Task, String> {
    if let Some(ticks) = dir.to_str().and_then(|s| s.strip_prefix(SMOKE_PREFIX)) {
        let ticks = ticks.parse().map_err(|_| format!("bad smoke task `{}`", dir.display()))?;
        return Ok(smoke_task(ticks));
    }
    let id = dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("task path has no name: {}", dir.display()))?
        .to_string();
    let path = dir.join("task.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_task(&id, dir, &text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The task path `smoke:<ticks>` names [`smoke_task`], so a child
/// grader can run it without a directory.
const SMOKE_PREFIX: &str = "smoke:";

/// The smoke task: run `ticks` ticks with no input; the only check is
/// that the program got there. The benchmark's loop uses it to tell a
/// model its program crashes.
pub fn smoke_task(ticks: u32) -> Task {
    Task {
        id: "smoke".into(),
        dir: PathBuf::from(format!("{SMOKE_PREFIX}{ticks}")),
        tier: 1,
        ticks,
        inputs: Vec::new(),
        checks: vec![Check { name: "runs".into(), at: ticks, expr: "true".into() }],
    }
}

pub fn parse_task(id: &str, dir: &Path, text: &str) -> Result<Task, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let int = |t: &toml::Table, key: &str| -> Result<Option<u32>, String> {
        match t.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_integer()
                .and_then(|n| u32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
        }
    };
    let strings = |t: &toml::Table, key: &str| -> Result<Vec<String>, String> {
        match t.get(key) {
            None => Ok(Vec::new()),
            Some(v) => v
                .as_array()
                .and_then(|a| a.iter().map(|s| s.as_str().map(str::to_string)).collect())
                .ok_or_else(|| format!("`{key}` must be a list of strings")),
        }
    };
    let ticks = int(&table, "ticks")?.ok_or("missing `ticks`")?;
    let tier = match int(&table, "tier")? {
        None => 1,
        Some(t @ 1..=3) => t as u8,
        Some(_) => return Err("`tier` must be 1, 2 or 3".into()),
    };
    let mut inputs = Vec::new();
    for (i, v) in table.get("input").and_then(|v| v.as_array()).into_iter().flatten().enumerate() {
        let t = v.as_table().ok_or(format!("input {i}: not a table"))?;
        let ctx = |e: String| format!("input {}: {e}", i + 1);
        let (from, to) = match (int(t, "at").map_err(ctx)?, int(t, "from").map_err(ctx)?, int(t, "to").map_err(ctx)?) {
            (Some(at), None, None) => (at, at + 1),
            (None, Some(from), Some(to)) if to > from => (from, to),
            _ => return Err(ctx("give `at`, or `from` and `to` (to > from)".into())),
        };
        let mouse = match t.get("mouse") {
            None => None,
            Some(v) => {
                let xy: Option<Vec<f64>> = v.as_array().and_then(|a| {
                    a.iter().map(|n| n.as_float().or_else(|| n.as_integer().map(|i| i as f64))).collect()
                });
                match xy.as_deref() {
                    Some([x, y]) => Some((*x, *y)),
                    _ => return Err(ctx("`mouse` must be [x, y]".into())),
                }
            }
        };
        let span = InputSpan {
            from,
            to,
            hold: strings(t, "hold").map_err(ctx)?,
            press: strings(t, "press").map_err(ctx)?,
            mouse,
            hold_mouse: strings(t, "hold_mouse").map_err(ctx)?,
            click: strings(t, "click").map_err(ctx)?,
        };
        if (!span.press.is_empty() || !span.click.is_empty()) && span.to != span.from + 1 {
            return Err(ctx("`press` and `click` happen on one tick: use `at`".into()));
        }
        if span.to > ticks {
            return Err(ctx(format!("runs past the last tick ({ticks})")));
        }
        inputs.push(span);
    }
    let mut checks = Vec::new();
    for (i, v) in table.get("check").and_then(|v| v.as_array()).into_iter().flatten().enumerate() {
        let t = v.as_table().ok_or(format!("check {i}: not a table"))?;
        let ctx = |e: String| format!("check {}: {e}", i + 1);
        let expr = t.get("expr").and_then(|v| v.as_str()).ok_or_else(|| ctx("missing `expr`".into()))?;
        let at = int(t, "at").map_err(ctx)?.unwrap_or(ticks);
        if at > ticks {
            return Err(ctx(format!("`at` is past the last tick ({ticks})")));
        }
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or(expr).to_string();
        checks.push(Check { name, at, expr: expr.to_string() });
    }
    if checks.is_empty() {
        return Err("a task needs at least one [[check]]".into());
    }
    Ok(Task { id: id.to_string(), dir: dir.to_path_buf(), tier, ticks, inputs, checks })
}

/// The input command for `tick`. The mouse stays where the latest
/// span that started by `tick` put it, as a real mouse does.
pub fn command_at(task: &Task, tick: u32) -> InputCommand {
    let mut cmd = InputCommand::default();
    let add = |list: &mut Vec<String>, names: &[String]| {
        for n in names {
            if !list.contains(n) {
                list.push(n.clone());
            }
        }
    };
    for s in task.inputs.iter().filter(|s| (s.from..s.to).contains(&tick)) {
        add(&mut cmd.keys_held, &s.hold);
        add(&mut cmd.keys_held, &s.press);
        add(&mut cmd.keys_pressed, &s.press);
        add(&mut cmd.mb_held, &s.hold_mouse);
        add(&mut cmd.mb_held, &s.click);
        add(&mut cmd.mb_press, &s.click);
    }
    if let Some((x, y)) = task
        .inputs
        .iter()
        .filter(|s| s.from <= tick)
        .filter_map(|s| s.mouse.map(|m| (s.from, m)))
        .max_by_key(|(from, _)| *from)
        .map(|(_, m)| m)
    {
        cmd.mouse_x = x;
        cmd.mouse_y = y;
    }
    cmd
}

// ---------------------------------------------------------------------------
// Grading
// ---------------------------------------------------------------------------

/// How far a program got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Lex,
    Parse,
    /// Top-level statements (including the scope check).
    Load,
    /// A runtime error on some tick.
    Run,
    /// Ran every tick; the checks decide.
    Checks,
    /// Killed at the wall-clock limit.
    Timeout,
    /// The grader itself failed (a crash, unreadable output).
    Crash,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Lex => "lex",
            Stage::Parse => "parse",
            Stage::Load => "load",
            Stage::Run => "run",
            Stage::Checks => "checks",
            Stage::Timeout => "timeout",
            Stage::Crash => "crash",
        }
    }
    fn parse(s: &str) -> Stage {
        match s {
            "lex" => Stage::Lex,
            "parse" => Stage::Parse,
            "load" => Stage::Load,
            "run" => Stage::Run,
            "checks" => Stage::Checks,
            "timeout" => Stage::Timeout,
            _ => Stage::Crash,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckOutcome {
    pub name: String,
    pub passed: bool,
    /// The value the expression gave, or why it gave none.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grade {
    pub passed: bool,
    pub stage: Stage,
    pub error: Option<String>,
    pub checks: Vec<CheckOutcome>,
    pub ticks_run: u32,
    pub stdout: String,
}

impl Grade {
    fn failed(stage: Stage, error: String, task: &Task) -> Grade {
        Grade {
            passed: false,
            stage,
            checks: task
                .checks
                .iter()
                .map(|c| CheckOutcome { name: c.name.clone(), passed: false, detail: "not reached".into() })
                .collect(),
            error: Some(error),
            ticks_run: 0,
            stdout: String::new(),
        }
    }

    pub fn to_json(&self) -> Json {
        json!({
            "passed": self.passed,
            "stage": self.stage.as_str(),
            "error": self.error,
            "ticks_run": self.ticks_run,
            "checks": self.checks.iter().map(|c| json!({
                "name": c.name, "passed": c.passed, "detail": c.detail,
            })).collect::<Vec<_>>(),
            "stdout": self.stdout,
        })
    }

    pub fn from_json(v: &Json) -> Option<Grade> {
        Some(Grade {
            passed: v["passed"].as_bool()?,
            stage: Stage::parse(v["stage"].as_str()?),
            error: v["error"].as_str().map(str::to_string),
            ticks_run: v["ticks_run"].as_u64()? as u32,
            checks: v["checks"]
                .as_array()?
                .iter()
                .map(|c| {
                    Some(CheckOutcome {
                        name: c["name"].as_str()?.to_string(),
                        passed: c["passed"].as_bool()?,
                        detail: c["detail"].as_str()?.to_string(),
                    })
                })
                .collect::<Option<_>>()?,
            stdout: v["stdout"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// The names of the checks that failed.
    pub fn failed_checks(&self) -> Vec<&str> {
        self.checks.iter().filter(|c| !c.passed).map(|c| c.name.as_str()).collect()
    }
}

/// Grade `source` on `task`, in this process. Fine for trusted
/// programs (tests, reference solutions); untrusted ones go through
/// [`grade_in_child`].
pub fn grade(task: &Task, source: &str) -> Grade {
    let tokens = match crate::lexer::lex(source) {
        Ok(t) => t,
        Err(e) => return Grade::failed(Stage::Lex, e.to_string(), task),
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => return Grade::failed(Stage::Parse, e.to_string(), task),
    };
    let mut env = Env::new();
    crate::stdlib::install(&mut env);
    eval::headless(&mut env);
    if let Err(e) = eval::run_top_level(&mut env, &program) {
        return Grade::failed(Stage::Load, e.to_string(), task);
    }

    let mut outcomes: Vec<Option<CheckOutcome>> = vec![None; task.checks.len()];
    let mut probe_id = 0;
    let mut run_checks = |env: &mut Env, tick: u32, outcomes: &mut Vec<Option<CheckOutcome>>| {
        for (i, c) in task.checks.iter().enumerate().filter(|(_, c)| c.at == tick) {
            let (passed, detail) = match probe(env, &c.expr, probe_id) {
                Ok((true, shown)) => (true, shown),
                Ok((false, shown)) => (false, shown),
                Err(e) => (false, e),
            };
            probe_id += 1;
            outcomes[i] = Some(CheckOutcome { name: c.name.clone(), passed, detail });
        }
    };
    run_checks(&mut env, 0, &mut outcomes);
    let mut error = None;
    let mut ticks_run = 0;
    for tick in 0..task.ticks {
        crate::host3d::apply_command(&mut env, &command_at(task, tick));
        let result = eval::tick_frame(&mut env, eval::PHYSICS_DT).and_then(|_| eval::render_frame3d(&mut env));
        if env.out.len() > STDOUT_CAP * 2 {
            let mut keep = STDOUT_CAP;
            while !env.out.is_char_boundary(keep) {
                keep -= 1;
            }
            env.out.truncate(keep);
        }
        if let Err(e) = result {
            error = Some(format!("tick {tick}: {e}"));
            break;
        }
        ticks_run = tick + 1;
        run_checks(&mut env, ticks_run, &mut outcomes);
    }
    let checks: Vec<CheckOutcome> = outcomes
        .into_iter()
        .zip(&task.checks)
        .map(|(o, c)| {
            o.unwrap_or_else(|| CheckOutcome { name: c.name.clone(), passed: false, detail: "not reached".into() })
        })
        .collect();
    let mut stdout = std::mem::take(&mut env.out);
    if stdout.len() > STDOUT_CAP {
        let mut keep = STDOUT_CAP;
        while !stdout.is_char_boundary(keep) {
            keep -= 1;
        }
        stdout.truncate(keep);
    }
    Grade {
        passed: error.is_none() && checks.iter().all(|c| c.passed),
        stage: if error.is_some() { Stage::Run } else { Stage::Checks },
        error,
        checks,
        ticks_run,
        stdout,
    }
}

/// Evaluate a check's expression in the running world: it runs as a
/// top-level `let`, so it sees the program's globals, classes and the
/// stdlib exactly as top-level code does. `Ok((is true, shown value))`.
fn probe(env: &mut Env, expr: &str, id: usize) -> Result<(bool, String), String> {
    let name = format!("__twe_probe_{id}");
    let source = format!("let {name} = ({expr})\n");
    let tokens = crate::lexer::lex(&source).map_err(|e| format!("check does not lex: {e}"))?;
    let program = crate::parser::parse(&tokens).map_err(|e| format!("check does not parse: {e}"))?;
    let out_before = env.out.len();
    let result = eval::run_top_level(env, &program);
    env.out.truncate(out_before);
    result.map_err(|e| e.message)?;
    let v = env.get(&name).ok_or("check left no value")?;
    let shown = v.display();
    Ok((v.is_bool() && v.as_bool(), shown))
}

// ---------------------------------------------------------------------------
// Grading in a child process
// ---------------------------------------------------------------------------

/// Grade `source` by running `exe bench grade <task> - --json` with the
/// source on stdin, killing it after `limit`. A timeout or a crash is a
/// failed grade, not an error.
pub fn grade_in_child(exe: &Path, task: &Task, source: &str, limit: Duration) -> Grade {
    let spawned = Command::new(exe)
        .args(["bench", "grade"])
        .arg(&task.dir)
        .args(["-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return Grade::failed(Stage::Crash, format!("starting the grader failed: {e}"), task),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(source.as_bytes());
    }
    // Drain stdout on a thread so a chatty program can't fill the pipe
    // and stall while we wait.
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(out) = stdout.as_mut() {
            let _ = std::io::Read::read_to_string(out, &mut s);
        }
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Grade::failed(Stage::Crash, format!("waiting on the grader failed: {e}"), task),
        }
    };
    let out = reader.join().unwrap_or_default();
    let Some(status) = status else {
        return Grade::failed(Stage::Timeout, format!("killed after {:.0} s", limit.as_secs_f64()), task);
    };
    match serde_json::from_str::<Json>(&out).ok().as_ref().and_then(Grade::from_json) {
        Some(g) => g,
        None => {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut e, &mut err);
            }
            let err: String = err.trim().chars().take(400).collect();
            Grade::failed(Stage::Crash, format!("grader exited with {status}: {err}"), task)
        }
    }
}

// ---------------------------------------------------------------------------
// Task validation
// ---------------------------------------------------------------------------

/// A broken variant of the reference solution.
#[derive(Debug, Clone)]
pub struct Mutant {
    pub what: String,
    pub source: String,
}

/// Mechanical mutants of `source`: each deletes one statement line,
/// flips one operator, or changes one number. Only mutants that still
/// parse are returned; a few may behave identically (an "equivalent
/// mutant"), which is fine — validation asks only that each check be
/// caught by *some* mutant.
pub fn mutants(source: &str) -> Vec<Mutant> {
    const FLIPS: &[(&str, &str)] = &[
        ("<=", ">"),
        (">=", "<"),
        ("==", "!="),
        ("!=", "=="),
        ("+=", "-="),
        ("-=", "+="),
        ("<", ">="),
        (">", "<="),
        (" + ", " - "),
        (" - ", " + "),
        (" * ", " / "),
        (" and ", " or "),
        (" or ", " and "),
        ("true", "false"),
        ("false", "true"),
    ];
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let with_line = |i: usize, new: Option<&str>| -> String {
        let mut v: Vec<&str> = Vec::with_capacity(lines.len());
        for (j, l) in lines.iter().enumerate() {
            if j == i {
                if let Some(n) = new {
                    v.push(n);
                }
            } else {
                v.push(l);
            }
        }
        v.join("\n") + "\n"
    };
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let n = i + 1;
        if !t.ends_with(':') {
            out.push(Mutant { what: format!("line {n}: deleted"), source: with_line(i, None) });
        }
        let code = code_ranges(line);
        let mut taken: Vec<(usize, usize)> = Vec::new();
        for (from, to) in FLIPS {
            let mut start = 0;
            while let Some(off) = line[start..].find(from) {
                let at = start + off;
                let end = at + from.len();
                start = end;
                let overlaps = taken.iter().any(|&(a, b)| at < b && end > a);
                if overlaps || !code.iter().any(|&(a, b)| at >= a && end <= b) {
                    continue;
                }
                let word = from.chars().all(|c| c.is_ascii_alphabetic());
                let boundary = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                if word && !(boundary(line[..at].chars().last()) && boundary(line[end..].chars().next())) {
                    continue;
                }
                // `->` (transitions, return types) is not a comparison.
                if *from == ">" && line[..at].ends_with('-') {
                    continue;
                }
                taken.push((at, end));
                let mutated = format!("{}{}{}", &line[..at], to, &line[end..]);
                out.push(Mutant {
                    what: format!("line {n}: `{}` → `{}`", from.trim(), to.trim()),
                    source: with_line(i, Some(&mutated)),
                });
            }
        }
        for (at, end) in numbers(line, &code) {
            let lit = &line[at..end];
            let new = if lit.contains('.') {
                let v: f64 = lit.parse().unwrap_or(0.0);
                if v == 0.0 { "1.0".to_string() } else { format!("{:?}", v * 2.0) }
            } else {
                let v: i64 = lit.parse().unwrap_or(0);
                if v == 0 { "1".to_string() } else { (v * 2).to_string() }
            };
            let mutated = format!("{}{}{}", &line[..at], new, &line[end..]);
            out.push(Mutant { what: format!("line {n}: {lit} → {new}"), source: with_line(i, Some(&mutated)) });
        }
    }
    out.retain(|m| {
        crate::lexer::lex(&m.source)
            .ok()
            .is_some_and(|t| crate::parser::parse(&t).is_ok())
    });
    out
}

/// Byte ranges of `line` that are code: outside string literals and
/// before a `#` comment.
fn code_ranges(line: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
                start = i + 1;
            }
        } else if c == '"' {
            ranges.push((start, i));
            in_str = true;
        } else if c == '#' {
            ranges.push((start, i));
            return ranges;
        }
    }
    if !in_str {
        ranges.push((start, line.len()));
    }
    ranges
}

/// Numeric literals in the code parts of `line` (not digits inside
/// identifiers like `x2`).
fn numbers(line: &str, code: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    for &(a, b) in code {
        let mut i = a;
        while i < b {
            let starts = bytes[i].is_ascii_digit()
                && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'.'));
            if !starts {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < b && (bytes[j].is_ascii_digit() || (bytes[j] == b'.' && j + 1 < b && bytes[j + 1].is_ascii_digit())) {
                j += 1;
            }
            out.push((i, j));
            i = j;
        }
    }
    out
}

/// One check's strength: how many runnable mutants it caught.
#[derive(Debug, Clone)]
pub struct CheckStrength {
    pub name: String,
    pub killed: usize,
}

/// The result of validating a task.
#[derive(Debug, Clone)]
pub struct Validation {
    pub task: String,
    pub solution: Grade,
    pub starter: Option<Grade>,
    /// Mutants that ran every tick (the ones that can test a check).
    pub runnable: usize,
    pub mutants: usize,
    pub strengths: Vec<CheckStrength>,
    pub problems: Vec<String>,
}

impl Validation {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Validate `task`: the reference solution passes, the starter (if
/// any) fails, and every check is failed by at least one mutant of the
/// solution that runs to completion — so no check passes vacuously.
/// Mutants are graded in child processes `jobs` at a time.
pub fn validate(exe: &Path, task: &Task, limit: Duration, jobs: usize) -> Result<Validation, String> {
    let read = |name: &str| std::fs::read_to_string(task.dir.join(name));
    let solution_src = read("solution.twe").map_err(|e| format!("{}: solution.twe: {e}", task.id))?;
    let mut problems = Vec::new();
    let solution = grade_in_child(exe, task, &solution_src, limit);
    if !solution.passed {
        problems.push(format!(
            "the solution fails ({}{}): {}",
            solution.stage.as_str(),
            solution.error.as_deref().map(|e| format!(", {e}")).unwrap_or_default(),
            solution.failed_checks().join("; ")
        ));
    }
    let starter = match read("starter.twe") {
        Ok(src) => {
            let g = grade_in_child(exe, task, &src, limit);
            if g.passed {
                problems.push("the starter already passes".into());
            }
            Some(g)
        }
        Err(_) => None,
    };
    let mut all = mutants(&solution_src);
    if let Ok(dir) = std::fs::read_dir(task.dir.join("mutants")) {
        let mut hand: Vec<PathBuf> = dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "twe")).collect();
        hand.sort();
        for p in hand {
            let source = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            all.push(Mutant { what: format!("mutants/{}", p.file_name().unwrap_or_default().to_string_lossy()), source });
        }
    }
    let grades = grade_many(exe, task, &all, limit, jobs);
    let runnable: Vec<&Grade> = grades.iter().filter(|g| g.stage == Stage::Checks).collect();
    let strengths: Vec<CheckStrength> = task
        .checks
        .iter()
        .enumerate()
        .map(|(i, c)| CheckStrength {
            name: c.name.clone(),
            killed: runnable.iter().filter(|g| g.checks.get(i).is_some_and(|o| !o.passed)).count(),
        })
        .collect();
    for s in &strengths {
        if s.killed == 0 {
            problems.push(format!(
                "check `{}` is never failed by a running mutant: it may pass vacuously (add a hand-written mutant in mutants/ if the mechanical ones can't reach it)",
                s.name
            ));
        }
    }
    Ok(Validation {
        task: task.id.clone(),
        solution,
        starter,
        runnable: runnable.len(),
        mutants: all.len(),
        strengths,
        problems,
    })
}

/// Grade many programs in child processes, `jobs` at a time, in order.
pub fn grade_many(exe: &Path, task: &Task, programs: &[Mutant], limit: Duration, jobs: usize) -> Vec<Grade> {
    let jobs = jobs.max(1);
    let mut grades: Vec<Option<Grade>> = vec![None; programs.len()];
    for (chunk_i, chunk) in programs.chunks(jobs).enumerate() {
        let done: Vec<Grade> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|m| s.spawn(|| grade_in_child(exe, task, &m.source, limit)))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Grade::failed(Stage::Crash, "grader thread panicked".into(), task)))
                .collect()
        });
        for (k, g) in done.into_iter().enumerate() {
            grades[chunk_i * jobs + k] = Some(g);
        }
    }
    grades.into_iter().map(|g| g.expect("every program graded")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(toml: &str) -> Task {
        parse_task("t", Path::new("."), toml).unwrap()
    }

    #[test]
    fn inputs_compile_to_per_tick_commands() {
        let t = task(
            "ticks = 10\n\
             [[input]]\nfrom = 0\nto = 4\nhold = [\"d\"]\n\
             [[input]]\nat = 2\npress = [\"space\"]\n\
             [[input]]\nfrom = 5\nto = 7\nmouse = [10, 20.5]\nhold_mouse = [\"left\"]\n\
             [[check]]\nexpr = \"true\"\n",
        );
        let c2 = command_at(&t, 2);
        assert_eq!(c2.keys_held, vec!["d", "space"]);
        assert_eq!(c2.keys_pressed, vec!["space"]);
        assert!(command_at(&t, 3).keys_pressed.is_empty());
        assert!(command_at(&t, 4).keys_held.is_empty());
        let c5 = command_at(&t, 5);
        assert_eq!((c5.mouse_x, c5.mouse_y), (10.0, 20.5));
        assert_eq!(c5.mb_held, vec!["left"]);
        // The mouse stays where it was put; the button doesn't.
        let c8 = command_at(&t, 8);
        assert_eq!((c8.mouse_x, c8.mouse_y), (10.0, 20.5));
        assert!(c8.mb_held.is_empty());
        assert_eq!(command_at(&t, 4).mouse_x, 0.0);
        assert_eq!(t.checks[0].at, 10);
    }

    #[test]
    fn task_errors_name_the_entry() {
        let bad = |s: &str| parse_task("t", Path::new("."), s).unwrap_err();
        assert!(bad("ticks = 5\n").contains("at least one"));
        assert!(bad("ticks = 5\n[[input]]\nfrom = 0\nto = 9\nhold = [\"a\"]\n[[check]]\nexpr = \"true\"\n").contains("input 1"));
        assert!(bad("ticks = 5\n[[input]]\nfrom = 0\nto = 2\npress = [\"a\"]\n[[check]]\nexpr = \"true\"\n").contains("use `at`"));
        assert!(bad("ticks = 5\n[[check]]\nat = 6\nexpr = \"true\"\n").contains("check 1"));
    }

    const MOVER: &str = "\
var x = 0.0
var jumps = 0
on update(dt):
    if key_held(\"d\"):
        x += 1.0
    if key_pressed(\"space\"):
        jumps += 1
";

    #[test]
    fn grading_feeds_input_and_checks_at_their_ticks() {
        let t = task(
            "ticks = 30\n\
             [[input]]\nfrom = 0\nto = 10\nhold = [\"d\"]\n\
             [[input]]\nat = 20\npress = [\"space\"]\n\
             [[check]]\nname = \"start\"\nat = 0\nexpr = \"x == 0.0\"\n\
             [[check]]\nname = \"moved\"\nat = 10\nexpr = \"x == 10.0\"\n\
             [[check]]\nname = \"stopped\"\nexpr = \"x == 10.0 and jumps == 1\"\n",
        );
        let g = grade(&t, MOVER);
        assert!(g.passed, "{g:?}");
        assert_eq!(g.ticks_run, 30);
        // Holding for one tick too few is caught at tick 10.
        let short = task(
            "ticks = 30\n[[input]]\nfrom = 0\nto = 9\nhold = [\"d\"]\n\
             [[check]]\nname = \"moved\"\nat = 10\nexpr = \"x == 10.0\"\n",
        );
        let g = grade(&short, MOVER);
        assert!(!g.passed);
        assert_eq!(g.checks[0].detail, "false");
    }

    #[test]
    fn failures_report_their_stage() {
        let t = task("ticks = 5\n[[check]]\nexpr = \"x == 1\"\n");
        assert_eq!(grade(&t, "let x = \"open\n").stage, Stage::Lex);
        assert_eq!(grade(&t, "let = 1\n").stage, Stage::Parse);
        assert_eq!(grade(&t, "print(nope)\n").stage, Stage::Load);
        let run = grade(&t, "var x = 1\non update(dt):\n    x = x / missing_thing()\n");
        assert_eq!(run.stage, Stage::Load, "unknown names are caught before running");
        let g = grade(&t, "var x = 1\nvar l = [1]\non update(dt):\n    x = l[5]\n");
        assert_eq!(g.stage, Stage::Run);
        assert!(g.error.as_deref().unwrap().starts_with("tick 0:"), "{g:?}");
        assert_eq!(g.checks[0].detail, "not reached");
        // A check on a missing name fails with the reason, not a crash.
        let g = grade(&t, "var y = 1\n");
        assert!(!g.passed && g.checks[0].detail.contains("x"), "{g:?}");
        // A non-boolean check value fails.
        let g = grade(&task("ticks = 1\n[[check]]\nexpr = \"1\"\n"), "var y = 1\n");
        assert!(!g.passed);
    }

    #[test]
    fn grade_json_round_trips() {
        let t = task("ticks = 3\n[[check]]\nexpr = \"x == 1\"\n");
        let g = grade(&t, "var x = 1\nprint(\"hi\")\n");
        assert_eq!(Grade::from_json(&g.to_json()), Some(g));
    }

    #[test]
    fn mutants_touch_code_not_strings_or_comments() {
        let src = "var x = 2\n# a < b\non update(dt):\n    if x < 3 and true:\n        print(\"a + b < c\")\n";
        let ms = mutants(src);
        let whats: Vec<&str> = ms.iter().map(|m| m.what.as_str()).collect();
        assert!(whats.contains(&"line 1: 2 → 4"), "{whats:?}");
        assert!(whats.contains(&"line 4: `<` → `>=`"), "{whats:?}");
        assert!(whats.contains(&"line 4: `and` → `or`"), "{whats:?}");
        assert!(whats.contains(&"line 4: `true` → `false`"), "{whats:?}");
        assert!(whats.contains(&"line 5: deleted") || !whats.iter().any(|w| w.starts_with("line 5")), "{whats:?}");
        assert!(!whats.iter().any(|w| w.starts_with("line 2")), "comments are left alone");
        assert!(!ms.iter().any(|m| m.source.contains("a - b") || m.source.contains("b >= c")), "strings are left alone");
        // `->` is not a comparison.
        assert!(!mutants("state a:\n    -> b\n").iter().any(|m| m.what.contains('>')));
    }
}
