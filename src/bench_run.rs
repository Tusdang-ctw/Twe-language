//! web3d-M5 session 4: running the benchmark (`twec bench run`).
//!
//! For each task and sample, a model writes a program through the
//! `llm_loop` (up to `max_rounds` rounds, with the language's checker
//! and a smoke run as feedback), and the final program is graded on
//! the task's behaviour checks in a child process (`bench::grade_in_child`).
//!
//! A run directory holds everything needed to audit or re-grade it:
//!
//! ```text
//! bench/runs/<date>-<label>/
//!   run.json                 settings, provider, bench version
//!   samples.jsonl            one line per (task, sample)
//!   programs/<task>/<n>.twe  each final program
//!   transcripts/<task>/<n>/  the loop's per-round JSONL
//!   summary.json, summary.md the scorecard
//! ```
//!
//! **Statistics.** pass@k uses the unbiased estimator of Chen et al.
//! (2021): with n samples of which c pass, pass@k = 1 - C(n-c, k) / C(n, k),
//! averaged over tasks. Confidence intervals are 95% percentile
//! bootstrap intervals over tasks (10,000 resamples, fixed seed), so they
//! reflect how much the score depends on which tasks were chosen.
//!
//! **Caching.** Every reply is cached under a hash of the provider, the
//! sample number and the full request, so an interrupted run resumes
//! where it stopped and `regrade` never calls a model.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};
use twe_llm::{Provider, Reply, Request, StopReason, Usage};

use crate::bench::{self, Grade, Grader, Lang, Stage, Task};
use crate::llm_loop::{self, LoopOptions, StaticVerdict};

/// Bumped when tasks, prompts or grading change in a way that makes
/// results incomparable with earlier runs.
pub const BENCH_VERSION: &str = "v1";

/// Game time the smoke check runs a program for, with no input.
const SMOKE_TICKS: u32 = 120;

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Which language the model writes, and how its programs are graded.
    pub grader: Grader,
    pub samples: u32,
    /// Rounds per sample; 1 means a single attempt with no feedback.
    pub max_rounds: u32,
    /// Send `twec verify`'s diagnostics back between rounds.
    pub verify_feedback: bool,
    /// Run each candidate for 2 s of game time with no input and send
    /// any crash back between rounds.
    pub smoke_feedback: bool,
    /// Ground the model with the Twe primer and the stdlib manifest.
    pub primer: bool,
    pub jobs: usize,
    pub grade_limit: Duration,
    pub cache_dir: Option<PathBuf>,
    pub out_dir: PathBuf,
    pub max_tokens: u32,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            grader: Grader::twe(Path::new("twec")),
            samples: 5,
            max_rounds: 4,
            verify_feedback: true,
            smoke_feedback: true,
            primer: true,
            jobs: 4,
            grade_limit: Duration::from_secs(30),
            cache_dir: Some(PathBuf::from("bench/cache")),
            out_dir: PathBuf::from("bench/runs/latest"),
            max_tokens: 16_000,
        }
    }
}

// ---------------------------------------------------------------------------
// The prompt
// ---------------------------------------------------------------------------

/// How the program is run and graded, in the model's terms. The same
/// facts go to the Python baseline in its own terms.
const HARNESS_TWE: &str = "\
How your program will be run: in Twe's 3D runtime, at a fixed 60 ticks per second. \
Each tick, input is read, then `on update(dt)` and entity `update(dt)` methods run with dt = 1/60 s, \
then `on render():`. Input arrives through `key.<name>` (held) and `key_press.<name>` (pressed this tick), \
and `mouse.x`, `mouse.y`, `mouse_press.left`, `mouse_held.left`. Key names: a–z, space, enter, escape, \
up, down, left, right. The program is tested automatically: a script presses keys, and after given ticks \
it reads the names listed under Interface, so use exactly those names. \
Reply with the complete program in one ```twe fenced block.";

/// The same facts for the Python + pygame-ce baseline.
const HARNESS_PY: &str = "\
Write the program in Python 3 with pygame-ce (`import pygame`). Define a class `Game` with \
`__init__(self)` (the game starts here), `update(self, dt, events)` and `draw(self, screen)`. \
It is run at a fixed 60 ticks per second: each tick, `update` is called with dt = 1/60 s and the list \
of this tick's pygame events, then `draw` with a 640x480 pygame Surface. Do not write a main loop, \
open a window or call `pygame.init()`: the test harness does that. Input: `pygame.key.get_pressed()` \
for held keys; KEYDOWN events (or `pygame.key.get_just_pressed()`) for presses; `pygame.mouse.get_pos()`; \
MOUSEBUTTONDOWN events and `pygame.mouse.get_pressed()` for buttons. Keys used: a–z, space, enter \
(`K_RETURN`), escape, and the arrow keys. The program is tested automatically: a script presses keys, \
and after given ticks it reads the attributes listed under Interface from your `Game` instance, so use \
exactly those names. Reply with the complete program in one ```python fenced block.";

/// The compact stdlib manifest: every builtin's name and parameters,
/// grouped by category.
pub fn stdlib_listing() -> String {
    let mut by_cat: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in crate::stdlib::manifest() {
        by_cat
            .entry(s.category.clone())
            .or_default()
            .push(format!("{}({})", s.name, s.params.join(", ")));
    }
    let mut out = String::from("## Every stdlib builtin (name and parameters)\n\n");
    for (cat, names) in by_cat {
        out.push_str(&format!("- {cat}: {}\n", names.join(", ")));
    }
    out
}

/// The system prompt: for Twe, the primer and the stdlib listing
/// (unless `primer` is off), then how the program is run. For Python,
/// how the program is run (models know Python and pygame already).
pub fn system_prompt(lang: Lang, primer: bool) -> String {
    if lang == Lang::Python {
        return HARNESS_PY.to_string();
    }
    if primer {
        format!("{}\n\n{}\n\n{HARNESS_TWE}", crate::primer::guide(), stdlib_listing())
    } else {
        format!("You are writing a program in Twe, a game scripting language.\n\n{HARNESS_TWE}")
    }
}

/// The task as the model sees it: the shared text, and the interface
/// for its language.
pub fn task_prompt(task: &Task, lang: Lang) -> Result<String, String> {
    let read = |name: &str| {
        std::fs::read_to_string(task.dir.join(name)).map_err(|e| format!("{}: {name}: {e}", task.id))
    };
    Ok(format!("# Task\n\n{}\n# Interface\n\n{}", read("task.md")?, read(lang.interface_file())?))
}

// ---------------------------------------------------------------------------
// The response cache
// ---------------------------------------------------------------------------

/// Wraps a provider: replies are stored under a hash of (provider,
/// sample, request) and served from disk when present.
pub struct Cached<'a> {
    pub inner: &'a mut dyn Provider,
    pub dir: PathBuf,
    pub sample: u32,
    /// Replies served from the cache in this sample.
    pub hits: u32,
}

/// A 128-bit FNV-1a hash (two 64-bit lanes with different offsets), as
/// 32 hex digits. Not cryptographic; a stable cache key.
pub fn hash_hex(data: &str) -> String {
    let mut a: u64 = 0xcbf2_9ce4_8422_2325;
    let mut b: u64 = 0x6c62_272e_07bb_0142;
    for &byte in data.as_bytes() {
        a = (a ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        b = (b ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(5);
    }
    format!("{a:016x}{b:016x}")
}

fn reply_json(r: &Reply) -> Json {
    json!({
        "text": r.text, "stop": r.stop.as_str(), "model": r.model, "cost_usd": r.cost_usd,
        "usage": {"input": r.usage.input, "output": r.usage.output,
                  "cache_read": r.usage.cache_read, "cache_write": r.usage.cache_write},
    })
}

fn reply_from_json(v: &Json) -> Option<Reply> {
    let u = &v["usage"];
    Some(Reply {
        text: v["text"].as_str()?.to_string(),
        stop: StopReason::parse(v["stop"].as_str()?),
        model: v["model"].as_str().unwrap_or_default().to_string(),
        cost_usd: v["cost_usd"].as_f64(),
        usage: Usage {
            input: u["input"].as_u64().unwrap_or(0),
            output: u["output"].as_u64().unwrap_or(0),
            cache_read: u["cache_read"].as_u64().unwrap_or(0),
            cache_write: u["cache_write"].as_u64().unwrap_or(0),
        },
    })
}

impl Provider for Cached<'_> {
    fn complete(&mut self, request: &Request) -> Result<Reply, twe_llm::Error> {
        let key = hash_hex(&format!(
            "{}\u{0}{}\u{0}{}\u{0}{}",
            self.inner.id(),
            self.sample,
            request.max_tokens,
            request.flatten()
        ));
        let path = self.dir.join(format!("{key}.json"));
        if let Some(reply) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Json>(&s).ok())
            .as_ref()
            .and_then(reply_from_json)
        {
            self.hits += 1;
            return Ok(reply);
        }
        let reply = self.inner.complete(request)?;
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(&path, reply_json(&reply).to_string());
        Ok(reply)
    }

    fn id(&self) -> String {
        self.inner.id()
    }
}

// ---------------------------------------------------------------------------
// One sample
// ---------------------------------------------------------------------------

/// What one (task, sample) produced.
#[derive(Clone, Debug)]
pub struct SampleRecord {
    pub task: String,
    pub tier: u8,
    pub sample: u32,
    /// The run couldn't get a program (a provider error); excluded
    /// from the scores and reported separately.
    pub infra_error: Option<String>,
    pub grade: Option<Grade>,
    pub rounds: u32,
    /// Round 1's program didn't lex or parse.
    pub first_syntax_error: bool,
    /// Round 1's `twec verify` error count.
    pub first_verify_errors: usize,
    pub final_verify_errors: usize,
    pub stop_truncated: bool,
    pub usage: Usage,
    pub cost_usd: Option<f64>,
    pub wall_ms: u64,
    pub cache_hits: u32,
    /// The task started from a starter file (an edit task).
    pub edit: bool,
    /// The final program is nearly the reference solution: a sign the
    /// model has seen the benchmark (flagged, not excluded).
    pub copies_reference: bool,
}

impl SampleRecord {
    pub fn passed(&self) -> bool {
        self.grade.as_ref().is_some_and(|g| g.passed)
    }

    pub fn to_json(&self) -> Json {
        json!({
            "task": self.task, "tier": self.tier, "sample": self.sample,
            "passed": self.passed(),
            "infra_error": self.infra_error,
            "grade": self.grade.as_ref().map(Grade::to_json),
            "rounds": self.rounds,
            "first_syntax_error": self.first_syntax_error,
            "first_verify_errors": self.first_verify_errors,
            "final_verify_errors": self.final_verify_errors,
            "stop_truncated": self.stop_truncated,
            "usage": {"input": self.usage.input, "output": self.usage.output,
                      "cache_read": self.usage.cache_read, "cache_write": self.usage.cache_write},
            "cost_usd": self.cost_usd,
            "wall_ms": self.wall_ms,
            "cache_hits": self.cache_hits,
            "edit": self.edit,
            "copies_reference": self.copies_reference,
        })
    }

    pub fn from_json(v: &Json) -> Option<SampleRecord> {
        let u = &v["usage"];
        Some(SampleRecord {
            task: v["task"].as_str()?.to_string(),
            tier: v["tier"].as_u64()? as u8,
            sample: v["sample"].as_u64()? as u32,
            infra_error: v["infra_error"].as_str().map(str::to_string),
            grade: if v["grade"].is_null() { None } else { Grade::from_json(&v["grade"]) },
            rounds: v["rounds"].as_u64().unwrap_or(0) as u32,
            first_syntax_error: v["first_syntax_error"].as_bool().unwrap_or(false),
            first_verify_errors: v["first_verify_errors"].as_u64().unwrap_or(0) as usize,
            final_verify_errors: v["final_verify_errors"].as_u64().unwrap_or(0) as usize,
            stop_truncated: v["stop_truncated"].as_bool().unwrap_or(false),
            usage: Usage {
                input: u["input"].as_u64().unwrap_or(0),
                output: u["output"].as_u64().unwrap_or(0),
                cache_read: u["cache_read"].as_u64().unwrap_or(0),
                cache_write: u["cache_write"].as_u64().unwrap_or(0),
            },
            cost_usd: v["cost_usd"].as_f64(),
            wall_ms: v["wall_ms"].as_u64().unwrap_or(0),
            cache_hits: v["cache_hits"].as_u64().unwrap_or(0) as u32,
            edit: v["edit"].as_bool().unwrap_or(false),
            copies_reference: v["copies_reference"].as_bool().unwrap_or(false),
        })
    }
}

/// The smoke check: run `source` for 2 s of game time with no input in
/// a child process; `Some(message)` when it doesn't load, crashes or
/// hangs.
pub fn smoke_check(grader: &Grader, source: &str, limit: Duration) -> Option<String> {
    let task = bench::smoke_task(SMOKE_TICKS);
    let g = grader.grade(&task, source, limit);
    match g.stage {
        Stage::Checks => None,
        Stage::Timeout => Some(format!(
            "Running the program for {} seconds of game time with no input did not finish within {} s: it may loop forever.",
            SMOKE_TICKS / 60,
            limit.as_secs()
        )),
        stage => Some(format!(
            "Running the program for {} seconds of game time with no input failed ({}): {}",
            SMOKE_TICKS / 60,
            stage.as_str(),
            g.error.unwrap_or_default()
        )),
    }
}

/// The language's static check as the loop's verdict: `twec verify`
/// (its JSON v2 goes back), or for Python `compile()` and pyflakes.
fn static_verdict(grader: &Grader, source: &str, limit: Duration) -> StaticVerdict {
    let (errors, report) = grader.static_check(source, limit);
    let message = match grader.lang {
        Lang::Twe => format!(
            "`twec verify` rejected the program. Its diagnostics (JSON v2) follow; each carries `fix.edits`, \
             anchored replacements you can apply.\n\n{report}"
        ),
        Lang::Python => format!("Python's checks (`compile()` and pyflakes) found errors:\n\n{report}"),
    };
    StaticVerdict { errors, report, message }
}

/// Whether `source` doesn't even parse.
fn syntax_error(grader: &Grader, source: &str, limit: Duration) -> bool {
    match grader.lang {
        Lang::Twe => crate::lexer::lex(source)
            .ok()
            .and_then(|t| crate::parser::parse(&t).ok())
            .is_none(),
        Lang::Python => grader.static_check(source, limit).1.contains("SyntaxError"),
    }
}

/// How alike two programs are, ignoring comments and whitespace: the
/// Jaccard similarity of their character trigrams (1.0 = the same text).
pub fn similarity(a: &str, b: &str) -> f64 {
    let norm = |s: &str| -> Vec<char> {
        s.lines()
            .map(|l| l.split('#').next().unwrap_or(""))
            .flat_map(|l| l.chars().filter(|c| !c.is_whitespace()))
            .collect()
    };
    let grams = |c: &[char]| -> std::collections::HashSet<[char; 3]> {
        c.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
    };
    let (ga, gb) = (grams(&norm(a)), grams(&norm(b)));
    if ga.is_empty() && gb.is_empty() {
        return 1.0;
    }
    ga.intersection(&gb).count() as f64 / ga.union(&gb).count() as f64
}

/// A hash of everything that defines a task set for `lang`: each
/// task's text, interface and `task.toml`. Runs with different hashes
/// aren't comparable.
pub fn tasks_hash(tasks: &[Task], lang: Lang) -> String {
    let mut all = String::new();
    for t in tasks {
        for f in ["task.md", lang.interface_file(), "task.toml"] {
            all.push_str(&t.id);
            all.push_str(f);
            all.push_str(&std::fs::read_to_string(t.dir.join(f)).unwrap_or_default());
        }
    }
    hash_hex(&all)
}

/// Run one sample: the loop, then the grade.
pub fn run_sample(
    provider: &mut dyn Provider,
    task: &Task,
    sample: u32,
    options: &RunOptions,
) -> SampleRecord {
    let start = Instant::now();
    let mut record = SampleRecord {
        task: task.id.clone(),
        tier: task.tier,
        sample,
        infra_error: None,
        grade: None,
        rounds: 0,
        first_syntax_error: false,
        first_verify_errors: 0,
        final_verify_errors: 0,
        stop_truncated: false,
        usage: Usage::default(),
        cost_usd: None,
        wall_ms: 0,
        cache_hits: 0,
        edit: false,
        copies_reference: false,
    };
    let grader = &options.grader;
    let lang = grader.lang;
    let prompt = match task_prompt(task, lang) {
        Ok(p) => p,
        Err(e) => {
            record.infra_error = Some(e);
            return record;
        }
    };
    let loop_options = LoopOptions {
        max_rounds: options.max_rounds,
        trace_dir: Some(options.out_dir.join("transcripts").join(&task.id).join(sample.to_string())),
        source_path: None,
        log_prompts: true,
        system: system_prompt(lang, options.primer),
        starter: std::fs::read_to_string(task.dir.join(format!("starter.{}", lang.ext()))).unwrap_or_default(),
        max_tokens: options.max_tokens,
        verify_feedback: options.verify_feedback,
        lang: lang.fences()[0].to_string(),
        lang_aliases: lang.fences()[1..].iter().map(|s| s.to_string()).collect(),
    };
    let smoke = |source: &str| {
        if options.smoke_feedback {
            smoke_check(grader, source, options.grade_limit)
        } else {
            None
        }
    };
    let verdict = |source: &str| static_verdict(grader, source, options.grade_limit);
    let outcome = match &options.cache_dir {
        Some(dir) => {
            let mut cached = Cached { inner: provider, dir: dir.clone(), sample, hits: 0 };
            let outcome = llm_loop::run_loop_with(&mut cached, &prompt, &loop_options, &verdict, &smoke);
            record.cache_hits = cached.hits;
            outcome
        }
        None => llm_loop::run_loop_with(provider, &prompt, &loop_options, &verdict, &smoke),
    };
    record.wall_ms = start.elapsed().as_millis() as u64;
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            record.infra_error = Some(e);
            return record;
        }
    };
    record.rounds = outcome.rounds.len() as u32;
    if let Some(first) = outcome.rounds.first() {
        record.first_syntax_error = syntax_error(grader, &first.source, options.grade_limit);
        record.first_verify_errors = first.verify_errors;
    }
    if let Some(last) = outcome.rounds.last() {
        record.final_verify_errors = last.verify_errors;
    }
    record.stop_truncated = outcome.rounds.iter().any(|r| r.stop == StopReason::MaxTokens);
    record.usage = outcome.usage;
    record.cost_usd = outcome.cost_usd;
    let program_dir = options.out_dir.join("programs").join(&task.id);
    let _ = std::fs::create_dir_all(&program_dir);
    let _ = std::fs::write(program_dir.join(format!("{sample}.{}", lang.ext())), &outcome.final_source);
    record.edit = !loop_options.starter.is_empty();
    if let Ok(reference) = std::fs::read_to_string(task.dir.join(format!("solution.{}", lang.ext()))) {
        record.copies_reference = similarity(&outcome.final_source, &reference) >= 0.9;
    }
    record.grade = Some(grader.grade(task, &outcome.final_source, options.grade_limit));
    record
}

// ---------------------------------------------------------------------------
// A whole run
// ---------------------------------------------------------------------------

/// Run every (task, sample) not already in the run directory's
/// `samples.jsonl`, `jobs` at a time, appending each record as it
/// finishes, then write the summary. `make_provider` builds a provider
/// per worker. Returns the records of the whole run.
pub fn run(
    tasks: &[Task],
    make_provider: &(dyn Fn() -> Result<Box<dyn Provider>, String> + Sync),
    provider_id: &str,
    options: &RunOptions,
) -> Result<Vec<SampleRecord>, String> {
    std::fs::create_dir_all(&options.out_dir).map_err(|e| format!("{}: {e}", options.out_dir.display()))?;
    let settings = json!({
        "bench_version": BENCH_VERSION,
        "lang": options.grader.lang.as_str(),
        // The primer is frozen per bench version; its hash shows it.
        "system_prompt_hash": hash_hex(&system_prompt(options.grader.lang, options.primer)),
        "tasks_hash": tasks_hash(tasks, options.grader.lang),
        "provider": provider_id,
        "samples": options.samples,
        "max_rounds": options.max_rounds,
        "verify_feedback": options.verify_feedback,
        "smoke_feedback": options.smoke_feedback,
        "primer": options.primer && options.grader.lang == Lang::Twe,
        "max_tokens": options.max_tokens,
        "tasks": tasks.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        "twec": env!("CARGO_PKG_VERSION"),
    });
    let run_json = options.out_dir.join("run.json");
    if let Ok(old) = std::fs::read_to_string(&run_json) {
        let old: Json = serde_json::from_str(&old).unwrap_or(Json::Null);
        for key in ["bench_version", "lang", "system_prompt_hash", "tasks_hash", "provider", "max_rounds", "verify_feedback", "smoke_feedback", "primer", "max_tokens"] {
            if old[key] != settings[key] {
                return Err(format!(
                    "{} holds a run with different settings ({key}: {} vs {}); use another --out",
                    options.out_dir.display(),
                    old[key],
                    settings[key]
                ));
            }
        }
    }
    std::fs::write(&run_json, serde_json::to_string_pretty(&settings).unwrap_or_default())
        .map_err(|e| format!("{}: {e}", run_json.display()))?;

    let samples_path = options.out_dir.join("samples.jsonl");
    let mut records = read_samples(&samples_path);
    // Infrastructure failures are retried; finished samples are kept.
    records.retain(|r| r.infra_error.is_none());
    rewrite_samples(&samples_path, &records)?;
    let done: std::collections::HashSet<(String, u32)> = records.iter().map(|r| (r.task.clone(), r.sample)).collect();
    let mut queue: std::collections::VecDeque<(&Task, u32)> = std::collections::VecDeque::new();
    for sample in 0..options.samples {
        for task in tasks {
            if !done.contains(&(task.id.clone(), sample)) {
                queue.push_back((task, sample));
            }
        }
    }
    let total = queue.len();
    let queue = Mutex::new(queue);
    let shared = Mutex::new(records);
    let finished = Mutex::new(0usize);
    // Provider failures in a row: five mean something is wrong with the
    // provider rather than one request, so the run stops.
    let failures_in_a_row = Mutex::new(0u32);
    let fatal: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..options.jobs.max(1) {
            s.spawn(|| {
                let mut provider = match make_provider() {
                    Ok(p) => p,
                    Err(e) => {
                        *fatal.lock().unwrap() = Some(e);
                        return;
                    }
                };
                loop {
                    if fatal.lock().unwrap().is_some() {
                        return;
                    }
                    let Some((task, sample)) = queue.lock().unwrap().pop_front() else { return };
                    let record = run_sample(provider.as_mut(), task, sample, options);
                    {
                        let mut in_a_row = failures_in_a_row.lock().unwrap();
                        match &record.infra_error {
                            Some(e) => {
                                *in_a_row += 1;
                                // A configuration problem (no key, an unknown
                                // model, a bad request) fails every sample.
                                let config = e.contains("credentials")
                                    || e.contains("no HTTP client")
                                    || (e.contains("HTTP 4") && !e.contains("HTTP 429"));
                                if config || *in_a_row >= 5 {
                                    *fatal.lock().unwrap() = Some(format!("stopping the run: {e}"));
                                }
                            }
                            None => *in_a_row = 0,
                        }
                    }
                    let mut n = finished.lock().unwrap();
                    *n += 1;
                    eprintln!(
                        "[bench] {}/{} {} #{}: {}",
                        *n,
                        total,
                        record.task,
                        record.sample,
                        match (&record.infra_error, &record.grade) {
                            (Some(e), _) => format!("ERROR {e}"),
                            (None, Some(g)) if g.passed => format!("pass ({} rounds)", record.rounds),
                            (None, Some(g)) => format!("fail at {} ({} rounds)", g.stage.as_str(), record.rounds),
                            (None, None) => "no grade".into(),
                        }
                    );
                    drop(n);
                    let line = record.to_json().to_string();
                    let mut all = shared.lock().unwrap();
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&samples_path) {
                        let _ = writeln!(f, "{line}");
                    }
                    all.push(record);
                }
            });
        }
    });
    if let Some(e) = fatal.into_inner().unwrap() {
        return Err(e);
    }
    let records = shared.into_inner().unwrap();
    write_summary(&options.out_dir, tasks, &records, &settings)?;
    Ok(records)
}

pub fn read_samples(path: &Path) -> Vec<SampleRecord> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Json>(l).ok())
        .filter_map(|v| SampleRecord::from_json(&v))
        .collect()
}

fn rewrite_samples(path: &Path, records: &[SampleRecord]) -> Result<(), String> {
    let mut text = String::new();
    for r in records {
        text.push_str(&r.to_json().to_string());
        text.push('\n');
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Re-grade every program in a run directory with the current grader
/// (no model calls), then rewrite `samples.jsonl` and the summary.
/// `grader` must be for the run's language (`run.json`'s `lang`).
pub fn regrade(grader: &Grader, run_dir: &Path, tasks_root: &Path, limit: Duration, jobs: usize) -> Result<Vec<SampleRecord>, String> {
    let samples_path = run_dir.join("samples.jsonl");
    let mut records = read_samples(&samples_path);
    if records.is_empty() {
        return Err(format!("{}: no samples", samples_path.display()));
    }
    let settings: Json = std::fs::read_to_string(run_dir.join("run.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Json::Null);
    let mut tasks: BTreeMap<String, Task> = BTreeMap::new();
    for r in &records {
        if !tasks.contains_key(&r.task) {
            tasks.insert(r.task.clone(), bench::load_task(&tasks_root.join(&r.task))?);
        }
    }
    let todo: Vec<usize> = (0..records.len()).filter(|&i| records[i].infra_error.is_none()).collect();
    for chunk in todo.chunks(jobs.max(1)) {
        let grades: Vec<(usize, Grade)> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|&i| {
                    let r = &records[i];
                    let task = &tasks[&r.task];
                    let path = run_dir.join("programs").join(&r.task).join(format!("{}.{}", r.sample, grader.lang.ext()));
                    s.spawn(move || {
                        let source = std::fs::read_to_string(&path).unwrap_or_default();
                        (i, grader.grade(task, &source, limit))
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (i, g) in grades {
            records[i].grade = Some(g);
        }
    }
    rewrite_samples(&samples_path, &records)?;
    let task_list: Vec<Task> = tasks.into_values().collect();
    write_summary(run_dir, &task_list, &records, &settings)?;
    Ok(records)
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// The unbiased pass@k for one task with `n` samples of which `c` pass.
pub fn pass_at_k(n: u32, c: u32, k: u32) -> f64 {
    if k == 0 || k > n {
        return f64::NAN;
    }
    if n - c < k {
        return 1.0;
    }
    // 1 - C(n-c, k) / C(n, k) = 1 - prod_{i=n-c+1}^{n} (1 - k/i)
    let mut prod = 1.0;
    for i in (n - c + 1)..=n {
        prod *= 1.0 - f64::from(k) / f64::from(i);
    }
    1.0 - prod
}

/// A small deterministic generator for the bootstrap (SplitMix64).
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// The mean of `values` with a 95% percentile-bootstrap interval
/// (resampling the values with replacement).
pub fn mean_ci(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (f64::NAN, f64::NAN, f64::NAN);
    }
    let n = values.len();
    let mean = values.iter().sum::<f64>() / n as f64;
    let mut rng = SplitMix(0x5eed_7e57);
    let mut means: Vec<f64> = (0..10_000)
        .map(|_| (0..n).map(|_| values[(rng.next() % n as u64) as usize]).sum::<f64>() / n as f64)
        .collect();
    means.sort_by(|a, b| a.total_cmp(b));
    (mean, means[249], means[9_749])
}

/// Per-task results.
struct TaskScore {
    id: String,
    tier: u8,
    n: u32,
    c: u32,
}

/// Write `summary.json` and `summary.md` for a set of records.
pub fn write_summary(dir: &Path, tasks: &[Task], records: &[SampleRecord], settings: &Json) -> Result<Json, String> {
    let summary = summarize(tasks, records, settings);
    std::fs::write(dir.join("summary.json"), serde_json::to_string_pretty(&summary).unwrap_or_default())
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join("summary.md"), summary_markdown(&summary)).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(summary)
}

pub fn summarize(tasks: &[Task], records: &[SampleRecord], settings: &Json) -> Json {
    let graded: Vec<&SampleRecord> = records.iter().filter(|r| r.infra_error.is_none()).collect();
    let scores: Vec<TaskScore> = tasks
        .iter()
        .map(|t| {
            let mine: Vec<&&SampleRecord> = graded.iter().filter(|r| r.task == t.id).collect();
            TaskScore {
                id: t.id.clone(),
                tier: t.tier,
                n: mine.len() as u32,
                c: mine.iter().filter(|r| r.passed()).count() as u32,
            }
        })
        .filter(|s| s.n > 0)
        .collect();
    let n_min = scores.iter().map(|s| s.n).min().unwrap_or(0);
    let pass_k: Vec<Json> = (1..=n_min)
        .map(|k| {
            let per: Vec<f64> = scores.iter().map(|s| pass_at_k(s.n, s.c, k)).collect();
            let (m, lo, hi) = mean_ci(&per);
            json!({"k": k, "mean": m, "ci95": [lo, hi]})
        })
        .collect();
    let edit_tasks: std::collections::HashSet<&str> =
        graded.iter().filter(|r| r.edit).map(|r| r.task.as_str()).collect();
    let by_kind: Vec<Json> = [("new", false), ("edit", true)]
        .iter()
        .filter_map(|(kind, edit)| {
            let per: Vec<f64> = scores
                .iter()
                .filter(|s| edit_tasks.contains(s.id.as_str()) == *edit)
                .map(|s| f64::from(s.c) / f64::from(s.n))
                .collect();
            if per.is_empty() {
                return None;
            }
            let (m, lo, hi) = mean_ci(&per);
            Some(json!({"kind": kind, "tasks": per.len(), "pass_at_1": m, "ci95": [lo, hi]}))
        })
        .collect();
    let by_tier: Vec<Json> = (1..=3u8)
        .filter_map(|tier| {
            let per: Vec<f64> = scores.iter().filter(|s| s.tier == tier).map(|s| f64::from(s.c) / f64::from(s.n)).collect();
            if per.is_empty() {
                return None;
            }
            let (m, lo, hi) = mean_ci(&per);
            Some(json!({"tier": tier, "tasks": per.len(), "pass_at_1": m, "ci95": [lo, hi]}))
        })
        .collect();
    let rate = |f: &dyn Fn(&SampleRecord) -> bool| {
        if graded.is_empty() {
            f64::NAN
        } else {
            graded.iter().filter(|r| f(r)).count() as f64 / graded.len() as f64
        }
    };
    let passes: Vec<&&SampleRecord> = graded.iter().filter(|r| r.passed()).collect();
    let mut usage = Usage::default();
    for r in &graded {
        usage.add(r.usage);
    }
    let costs: Vec<f64> = graded.iter().filter_map(|r| r.cost_usd).collect();
    let mut stages: BTreeMap<String, usize> = BTreeMap::new();
    for r in &graded {
        if let Some(g) = &r.grade {
            *stages.entry(g.stage.as_str().to_string()).or_default() += 1;
        }
    }
    json!({
        "settings": settings,
        "samples": graded.len(),
        "infra_errors": records.len() - graded.len(),
        "tasks": scores.len(),
        "pass_at_k": pass_k,
        "by_tier": by_tier,
        "by_kind": by_kind,
        "copies_reference_rate": rate(&|r| r.copies_reference),
        "per_task": scores.iter().map(|s| json!({"task": s.id, "tier": s.tier, "n": s.n, "passed": s.c})).collect::<Vec<_>>(),
        "first_round_syntax_error_rate": rate(&|r| r.first_syntax_error),
        "first_round_verify_clean_rate": rate(&|r| !r.first_syntax_error && r.first_verify_errors == 0),
        "final_verify_clean_rate": rate(&|r| r.final_verify_errors == 0),
        "truncated_rate": rate(&|r| r.stop_truncated),
        "mean_rounds": if graded.is_empty() { f64::NAN } else { graded.iter().map(|r| f64::from(r.rounds)).sum::<f64>() / graded.len() as f64 },
        "mean_rounds_when_passed": if passes.is_empty() { f64::NAN } else { passes.iter().map(|r| f64::from(r.rounds)).sum::<f64>() / passes.len() as f64 },
        "final_stage": stages,
        "tokens": {"input": usage.input, "output": usage.output, "cache_read": usage.cache_read, "cache_write": usage.cache_write},
        "cost_usd": if costs.is_empty() { Json::Null } else { json!(costs.iter().sum::<f64>()) },
        "cost_per_sample_usd": if costs.is_empty() { Json::Null } else { json!(costs.iter().sum::<f64>() / costs.len() as f64) },
        "mean_wall_s": if graded.is_empty() { f64::NAN } else { graded.iter().map(|r| r.wall_ms as f64).sum::<f64>() / graded.len() as f64 / 1000.0 },
    })
}

fn pct(v: &Json) -> String {
    match v.as_f64() {
        Some(x) if x.is_finite() => format!("{:.1}%", x * 100.0),
        _ => "–".into(),
    }
}

pub fn summary_markdown(s: &Json) -> String {
    let set = &s["settings"];
    let mut out = format!(
        "# Bench {} — {}, {}\n\n{} tasks, {} samples ({} infrastructure errors excluded); up to {} rounds, checker feedback {}, smoke feedback {}, primer {}.\n\n",
        set["bench_version"].as_str().unwrap_or("?"),
        set["lang"].as_str().unwrap_or("twe"),
        set["provider"].as_str().unwrap_or("?"),
        s["tasks"],
        s["samples"],
        s["infra_errors"],
        set["max_rounds"],
        if set["verify_feedback"] == true { "on" } else { "off" },
        if set["smoke_feedback"] == true { "on" } else { "off" },
        if set["primer"] == true { "on" } else { "off" },
    );
    out.push_str("| | Score | 95% CI |\n|---|---:|---|\n");
    for p in s["pass_at_k"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "| pass@{} | {} | {} – {} |\n",
            p["k"],
            pct(&p["mean"]),
            pct(&p["ci95"][0]),
            pct(&p["ci95"][1])
        ));
    }
    for t in s["by_tier"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "| pass@1, tier {} ({} tasks) | {} | {} – {} |\n",
            t["tier"],
            t["tasks"],
            pct(&t["pass_at_1"]),
            pct(&t["ci95"][0]),
            pct(&t["ci95"][1])
        ));
    }
    for k in s["by_kind"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "| pass@1, {} tasks ({}) | {} | {} – {} |\n",
            k["kind"].as_str().unwrap_or("?"),
            k["tasks"],
            pct(&k["pass_at_1"]),
            pct(&k["ci95"][0]),
            pct(&k["ci95"][1])
        ));
    }
    out.push_str(&format!(
        "\n- Final programs nearly identical to the reference solution (a contamination sign): {}.\n",
        pct(&s["copies_reference_rate"])
    ));
    out.push_str(&format!(
        "- Round 1: {} didn't parse; {} were checker-clean. Final programs checker-clean: {}.\n",
        pct(&s["first_round_syntax_error_rate"]),
        pct(&s["first_round_verify_clean_rate"]),
        pct(&s["final_verify_clean_rate"])
    ));
    out.push_str(&format!(
        "- Rounds: {:.2} on average, {:.2} for passing samples. Replies cut off at the token limit: {}.\n",
        s["mean_rounds"].as_f64().unwrap_or(f64::NAN),
        s["mean_rounds_when_passed"].as_f64().unwrap_or(f64::NAN),
        pct(&s["truncated_rate"])
    ));
    let tok = &s["tokens"];
    out.push_str(&format!(
        "- Tokens: {} in, {} out, {} cache read, {} cache write. Cost: {}{}. Mean wall time {:.1} s per sample.\n",
        tok["input"],
        tok["output"],
        tok["cache_read"],
        tok["cache_write"],
        s["cost_usd"].as_f64().map(|c| format!("${c:.2}")).unwrap_or_else(|| "unknown".into()),
        s["cost_per_sample_usd"].as_f64().map(|c| format!(" (${c:.4} per sample)")).unwrap_or_default(),
        s["mean_wall_s"].as_f64().unwrap_or(f64::NAN),
    ));
    out.push_str("- Where final programs ended: ");
    let stages: Vec<String> = s["final_stage"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k} {v}"))
        .collect();
    out.push_str(&stages.join(", "));
    out.push_str(" (`checks` means it ran every tick; pass or fail is then the checks').\n\n| Task | Tier | Passed |\n|---|---:|---:|\n");
    for t in s["per_task"].as_array().into_iter().flatten() {
        out.push_str(&format!("| `{}` | {} | {}/{} |\n", t["task"].as_str().unwrap_or("?"), t["tier"], t["passed"], t["n"]));
    }
    out
}

/// Today's date (UTC) as `YYYY-MM-DD`, for run directory names.
pub fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_at_k_matches_the_definition() {
        // n = 5, c = 2: pass@1 = 2/5; pass@2 = 1 - C(3,2)/C(5,2) = 1 - 3/10.
        assert!((pass_at_k(5, 2, 1) - 0.4).abs() < 1e-12);
        assert!((pass_at_k(5, 2, 2) - 0.7).abs() < 1e-12);
        assert_eq!(pass_at_k(5, 0, 3), 0.0);
        assert_eq!(pass_at_k(5, 3, 3), 1.0);
        assert_eq!(pass_at_k(5, 5, 1), 1.0);
    }

    #[test]
    fn bootstrap_interval_brackets_the_mean() {
        let values = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0, 1.0, 0.0, 0.4, 0.6];
        let (m, lo, hi) = mean_ci(&values);
        assert!((m - 0.5).abs() < 1e-12);
        assert!(lo < m && m < hi && lo > 0.2 && hi < 0.8, "{lo} {hi}");
        // Deterministic.
        assert_eq!(mean_ci(&values), (m, lo, hi));
        let (_, lo, hi) = mean_ci(&[1.0; 8]);
        assert_eq!((lo, hi), (1.0, 1.0));
    }

    #[test]
    fn similarity_ignores_comments_and_layout() {
        assert_eq!(similarity("var x = 1\nprint(x)", "var x = 1   # one\n\nprint( x )"), 1.0);
        assert!(similarity("var x = 1\nprint(x)", "let total = 0\nfor i in 0..<9:\n    total += i") < 0.2);
    }

    #[test]
    fn cache_keys_are_stable_and_distinct() {
        assert_eq!(hash_hex("abc"), hash_hex("abc"));
        assert_ne!(hash_hex("abc"), hash_hex("abd"));
        assert_eq!(hash_hex("").len(), 32);
    }

    #[test]
    fn the_prompt_carries_the_primer_the_stdlib_and_the_harness() {
        let with = system_prompt(Lang::Twe, true);
        assert!(with.contains("Golden rules"));
        assert!(with.contains("math.sqrt("));
        assert!(with.contains("60 ticks per second"));
        let without = system_prompt(Lang::Twe, false);
        assert!(!without.contains("Golden rules") && without.contains("60 ticks per second"));
        let python = system_prompt(Lang::Python, true);
        assert!(python.contains("class `Game`") && !python.contains("Golden rules"));
    }
}
