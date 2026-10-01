use std::env;
use std::fs;
use std::process;

const USAGE: &str = "usage: twec [run [--frames N] <file> | \
     play <file> | \
     play3d <file> | \
     play_visual <file> | \
     replay <script.twe> <replay-file> | \
     profile [--frames N] [-o trace.json] <file> | \
     build [--target T] [--config C] [--out PATH] [--dry-run] [--steam] <project_dir> | \
     bundle [-o PATH] <project_dir> | \
     info <bundle-or-exe> | \
     verify [--warn-deprecated] <file> | \
     grammar [--format gbnf|json-schema|ebnf] [-o PATH] | \
     stdlib [--json] [--category NAME] [-o PATH] | \
     llm-loop (--provider anthropic|openai --model M [--effort E] [--base-url URL] | --command CMD [--arg ARG]*) [--prompt PATH] [--starter PATH] [--no-primer] [--max-rounds N] [--out PATH] [--trace-dir DIR] | \
     bench grade <task-dir> <file|-> [--json] | bench check [--all | <task-dir>...] [--jobs N] [--timeout S] | bench run <provider flags> [--samples N] [--rounds N] [--no-verify] [--no-smoke] [--no-primer] [--tasks a,b] [--out DIR] | bench regrade <run-dir> | \
     mcp | \
     corpus [--json] [-o PATH] | \
     eval [SUITE] [--source FILE] [--source-dir DIR] [--root DIR] [--json] [-o PATH] | \
     mutate [--root DIR] [--out DIR] [--rules RULESET] | \
     perf-snapshot [--target DIR] [-o PATH] | \
     perf-diff [--threshold PCT] <baseline.json> <current.json> | \
     doctor [--json] [-o PATH] | \
     fmt [--in-place|--check] <file> | \
     types <file> | lsp | parse <file> | version]";

/// web3d-M1: the bytecode VM was removed (see
/// docs/changes/2026-09-28-web3d-m1-closeout.md). `--vm tree` is still
/// accepted as a no-op so existing scripts and CI invocations keep
/// working; `--vm bytecode` explains the removal.
const VM_REMOVED: &str = "error: the bytecode VM was removed in web3d-M1 \
     (docs/changes/2026-09-28-web3d-m1-closeout.md); the tree-walker is Twe's only runtime \
     — drop `--vm bytecode`";

pub fn run() {
    install_crash_reporter();
    // Phase 12 session 4: if our binary has a bundle appended (via
    // `twec build --target windows-x86_64`), launch the embedded
    // game directly. The user double-clicked their `survive.exe`,
    // not the Twe CLI. This path runs *before* arg parsing so an
    // embedded-bundle binary ignores stray launcher arguments
    // Steam / shells sometimes pass.
    match crate::bundle::detect_in_self() {
        Ok(Some(reader)) => {
            process::exit(run_embedded(reader));
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("[twec] warning: could not check for embedded bundle: {e}");
            // Fall through to normal CLI. Don't kill the process —
            // a transient `current_exe` failure shouldn't break the
            // contributor's local `cargo run`.
        }
    }
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_version();
        return;
    }
    if matches!(
        args[1].as_str(),
        "run" | "play" | "play3d" | "play_visual" | "profile"
    ) {
        register_asset_root(&args[2..]);
    }
    match args[1].as_str() {
        "run" => process::exit(handle_run(&args[2..])),
        "play" => process::exit(handle_play(&args[2..])),
        "profile" => process::exit(handle_profile(&args[2..])),
        "build" => process::exit(handle_build(&args[2..])),
        "bundle" => process::exit(handle_bundle(&args[2..])),
        "info" => process::exit(handle_info(&args[2..])),
        "verify" => process::exit(handle_verify(&args[2..])),
        // Phase 33 session 1: portable grammar export — the LLM
        // contract surface. `twec grammar --format gbnf` produces a
        // GBNF file consumable by llama.cpp constrained decoding.
        "grammar" => process::exit(handle_grammar(&args[2..])),
        // Phase 33 session 3: stdlib JSON manifest — every callable
        // enumerable with signature + category. The LLM is grounded
        // on this so API hallucination becomes mechanically impossible.
        "stdlib" => process::exit(handle_stdlib(&args[2..])),
        // Phase 33 session 4: end-to-end LLM authoring loop. Drives a
        // user-configured command provider through verify-feedback
        // rounds and logs JSONL traces (training-corpus seed).
        "llm-loop" | "llm_loop" => process::exit(handle_llm_loop(&args[2..])),
        // web3d-M5: the LLM benchmark (grade programs, validate tasks).
        "bench" => process::exit(handle_bench(&args[2..])),
        // Phase 33 session 5: stdio JSON-RPC MCP server. Every Twe
        // tool becomes available to any MCP client (Claude Desktop,
        // Cursor, the future Twe Studio) with no bespoke wiring.
        "mcp" => process::exit(handle_mcp(&args[2..])),
        // LLM grounding text for out-of-process clients (e.g. Twe Studio's
        // in-app AI prompt). `twec primer` prints the concise primer;
        // `twec primer --full` prints the complete guide.
        "primer" => process::exit(handle_primer(&args[2..])),
        // Phase 33 session 6: enumerate the labeled examples corpus
        // built from `@task / @inputs / @expected / @category` headers.
        "corpus" => process::exit(handle_corpus(&args[2..])),
        // Phase 33 session 7: grade an LLM-generated source against
        // a replay-based suite. Returns a JSON scorecard.
        "eval" => process::exit(handle_eval(&args[2..])),
        // Phase 33 session 8: auto-mutate tests/programs/ to produce
        // the fine-tune-ready (broken, verify_json, fix) corpus.
        "mutate" => process::exit(handle_mutate(&args[2..])),
        // Phase 35 session 1: snapshot the public API surface for
        // the 6-month stability audit. `twec api-snapshot -o PATH`
        // writes a canonical, hashable JSON document; `twec api-diff
        // <old> <new>` compares two snapshots and exits non-zero if
        // any surface has changed.
        "api-snapshot" | "api_snapshot" => process::exit(handle_api_snapshot(&args[2..])),
        "api-diff" | "api_diff" => process::exit(handle_api_diff(&args[2..])),
        // v1.0.1 session 11: perf-bench snapshot + diff. Scrapes
        // `target/criterion/` from `cargo bench` into a canonical JSON
        // document, and exits non-zero when a tracked bench regresses
        // beyond the threshold (default 5%).
        "perf-snapshot" | "perf_snapshot" => process::exit(handle_perf_snapshot(&args[2..])),
        "perf-diff" | "perf_diff" => process::exit(handle_perf_diff(&args[2..])),
        // v1.0.1 session 13: `twec doctor [--json] [-o PATH]` —
        // single-page environment + crash-history report for triage.
        "doctor" => process::exit(handle_doctor(&args[2..])),
        "play3d" => process::exit(handle_play3d(&args[2..])),
        // v1.0.1 session 10: replay a crash bundle (or any v1
        // replay log) by running a script under playback. The play
        // loop's existing `replay::tick` picks up the playing mode
        // and overwrites the input ambients per frame.
        "replay" => process::exit(handle_replay(&args[2..])),
        "play_visual" => process::exit(handle_play_visual(&args[2..])),
        "fmt" => process::exit(handle_fmt(&args[2..])),
        "lsp" => process::exit(handle_lsp(&args[2..])),
        "types" => process::exit(handle_types(&args[2..])),
        "parse" => process::exit(handle_parse(&args[2..])),
        "version" | "--version" | "-V" => print_version(),
        cmd => {
            eprintln!("error: unknown command '{cmd}'");
            eprintln!("{USAGE}");
            process::exit(2);
        }
    }
}

/// Phase 12 session 4: launch a self-extracting binary's embedded
/// game. Reads `main.twe` from the bundle, installs the bundle as
/// the active asset source, hands the source string to
/// `play::launch_embedded`. Returns the process exit code so
/// `cli::run`'s caller can `process::exit(...)`.
fn run_embedded(mut reader: crate::bundle::BundleReader) -> i32 {
    let main = match reader.read("main.twe") {
        Ok(Some(b)) => b,
        Ok(None) => {
            eprintln!("error: bundled game has no main.twe");
            return 1;
        }
        Err(e) => {
            eprintln!("error: could not read main.twe from bundle: {e}");
            return 1;
        }
    };
    let src = match String::from_utf8(main) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("error: main.twe in bundle is not valid UTF-8");
            return 1;
        }
    };
    crate::bundle::set_active_bundle(reader);
    let code = crate::play::launch_embedded(src);
    crate::play::shutdown_gilrs();
    code
}

fn handle_play(args: &[String]) -> i32 {
    let parsed = match parse_common_flags(args, false) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let path = match parsed.path {
        Some(p) => p,
        None => {
            eprintln!("error: `twec play` requires a file path");
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let code = crate::play::launch(path);
    crate::play::shutdown_gilrs();
    code
}

/// `twec play3d <file>` — wgpu-driven 3D backend (Phase 5 task 5
/// session 1: clear-color window). No `--vm` flag yet; the 3D
/// surface only runs the script's top-level code at startup, so
/// the choice of interpreter doesn't matter until the loop drives
/// per-frame work in a later session.
fn handle_play3d(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("error: `twec play3d` requires a file path");
        eprintln!("{USAGE}");
        return 2;
    }
    // Reject unknown flags so a typo doesn't get silently
    // interpreted as the file path.
    let mut path: Option<String> = None;
    for a in args {
        if a.starts_with('-') {
            eprintln!("error: unknown flag for `play3d`: {a}");
            eprintln!("{USAGE}");
            return 2;
        }
        if path.is_some() {
            eprintln!("error: `twec play3d` takes one file path");
            return 2;
        }
        path = Some(a.clone());
    }
    let path = path.expect("non-empty args + no flags ⇒ at least one positional");
    crate::play3d::launch(path)
}

/// v1.0.1 session 10: `twec replay <script.twe> <replay-file>` —
/// re-runs `script.twe` under the play loop with `replay-file`
/// driving every per-frame input. The standard crash-reporter hook
/// writes a sibling `twec-crash-<secs>-<pid>.replay` next to every
/// `.log`, so the canonical bug-bundle workflow is "user posts the
/// `.log` + `.replay` pair → maintainer runs `twec replay <repro-script>
/// <crash.replay>`." Returns the same exit code as the underlying
/// `play` invocation; replay halts automatically at end-of-file and
/// the script keeps running with no further input.
fn handle_replay(args: &[String]) -> i32 {
    let mut positional: Vec<&str> = Vec::new();
    for a in args {
        if a.starts_with('-') {
            eprintln!("error: unknown flag for `replay`: {a}");
            eprintln!("{USAGE}");
            return 2;
        }
        positional.push(a.as_str());
    }
    if positional.len() != 2 {
        eprintln!(
            "error: `twec replay` takes exactly two positional args: <script.twe> <replay-file>"
        );
        eprintln!("{USAGE}");
        return 2;
    }
    let script = positional[0].to_string();
    let replay_path = positional[1];
    if let Err(e) = crate::replay::start_playing(replay_path) {
        eprintln!("error: {e}");
        return 1;
    }
    let code = crate::play::launch(script);
    crate::replay::stop();
    crate::play::shutdown_gilrs();
    code
}

/// `twec build [--target T] [--config C] [--out PATH] [--dry-run]
/// <project_dir>` — Phase 12: produce a redistributable for a
/// project tree. Session 1 ships the validation skeleton
/// (`<dir>/main.twe` required + `<dir>/assets/` walked + optional
/// `twe.toml`); sessions 2+ fill in real bundle production +
/// per-target binary output.
fn handle_build(args: &[String]) -> i32 {
    use crate::build::{BuildArgs, BuildConfig, BuildTarget};
    let mut target: Option<BuildTarget> = None;
    let mut config: Option<BuildConfig> = None;
    let mut out: Option<std::path::PathBuf> = None;
    let mut dry_run = false;
    let mut steam = false;
    let mut project_dir: Option<std::path::PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("error: `--target` needs a value");
                    return 2;
                };
                let Some(t) = BuildTarget::parse(v) else {
                    eprintln!(
                        "error: unknown target '{v}' (try windows-x86_64, macos-aarch64, macos-x86_64, linux-x86_64, web, wasm32)"
                    );
                    return 2;
                };
                target = Some(t);
                i += 1;
            }
            "--config" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("error: `--config` needs a value");
                    return 2;
                };
                let Some(c) = BuildConfig::parse(v) else {
                    eprintln!("error: unknown config '{v}' (try dev, release, profile)");
                    return 2;
                };
                config = Some(c);
                i += 1;
            }
            "--out" | "-o" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("error: `--out` needs a path");
                    return 2;
                };
                out = Some(v.into());
                i += 1;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            "--steam" => {
                steam = true;
                i += 1;
            }
            other if other.starts_with("--") => {
                eprintln!("error: unknown flag '{other}'");
                eprintln!("{USAGE}");
                return 2;
            }
            other => {
                if project_dir.is_some() {
                    eprintln!("error: `twec build` takes a single project directory");
                    return 2;
                }
                project_dir = Some(other.into());
                i += 1;
            }
        }
    }
    let Some(project_dir) = project_dir else {
        eprintln!("error: `twec build` requires a project directory");
        eprintln!("{USAGE}");
        return 2;
    };
    let target_explicit = target.is_some();
    let config_explicit = config.is_some();
    let args = BuildArgs {
        project_dir,
        target: target.unwrap_or_else(BuildTarget::host),
        target_explicit,
        config: config.unwrap_or(BuildConfig::Release),
        config_explicit,
        out,
        dry_run,
        steam,
    };
    crate::build::run(args)
}

/// `twec bundle [-o PATH] <project_dir>` — Phase 12 session 2:
/// emit a standalone `.twebundle` artifact for inspection / hand-
/// shipping. Mirrors `twec build` discovery + validation but skips
/// the binary-production step. Useful for diff-friendly review of
/// what a build would package and for round-tripping through the
/// reader in tools.
fn handle_bundle(args: &[String]) -> i32 {
    let mut out: Option<std::path::PathBuf> = None;
    let mut project_dir: Option<std::path::PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("error: `-o` needs a path");
                    return 2;
                };
                out = Some(v.into());
                i += 1;
            }
            other if other.starts_with("--") || other == "-o" => {
                eprintln!("error: unknown flag '{other}'");
                eprintln!("{USAGE}");
                return 2;
            }
            other => {
                if project_dir.is_some() {
                    eprintln!("error: `twec bundle` takes a single project directory");
                    return 2;
                }
                project_dir = Some(other.into());
                i += 1;
            }
        }
    }
    let Some(project_dir) = project_dir else {
        eprintln!("error: `twec bundle` requires a project directory");
        eprintln!("{USAGE}");
        return 2;
    };
    let project = match crate::build::discover_project(&project_dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Err(e) = crate::build::validate_project(&project) {
        eprintln!("error: {e}");
        return 1;
    }
    let out_path = out.unwrap_or_else(|| {
        project
            .root
            .join("dist")
            .join(format!("{}.twebundle", project.name))
    });
    match crate::build::write_bundle(&project, &out_path) {
        Ok(bytes) => {
            eprintln!(
                "[twec bundle] wrote {} ({} bytes, {} entries)",
                out_path.display(),
                bytes,
                project.assets.len() + 1
            );
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// `twec info <path>` — Phase 12 session 10: print build provenance
/// + entry list for a `.twebundle` or self-extracting binary.
fn handle_info(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("error: `twec info` requires a path");
        eprintln!("{USAGE}");
        return 2;
    }
    let mut path: Option<&str> = None;
    for a in args {
        if a.starts_with('-') {
            eprintln!("error: unknown flag for `info`: {a}");
            eprintln!("{USAGE}");
            return 2;
        }
        if path.is_some() {
            eprintln!("error: `twec info` takes one path");
            return 2;
        }
        path = Some(a.as_str());
    }
    let path = path.expect("non-empty args + no flags ⇒ at least one positional");
    crate::build::run_info(std::path::Path::new(path))
}

/// `twec verify <file>` — Phase 13 session 8. Tier 3 LLM-facing
/// reporter. Runs the file through lex + parse + strict-lax
/// inference (the same pipeline `# verified` activates from inside
/// the source) and emits the canonical JSON report on stdout. Exit
/// code is 0 when the report has no errors, 1 otherwise — suitable
/// for an LLM self-correction loop or a CI gate.
fn handle_verify(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("error: `twec verify` requires a file path");
        eprintln!("{USAGE}");
        return 2;
    }
    let mut path: Option<&str> = None;
    let mut warn_deprecated = false;
    for a in args {
        match a.as_str() {
            "--warn-deprecated" => {
                warn_deprecated = true;
            }
            s if s.starts_with('-') => {
                eprintln!("error: unknown flag for `verify`: {a}");
                eprintln!("{USAGE}");
                return 2;
            }
            _ => {
                if path.is_some() {
                    eprintln!("error: `twec verify` takes one file path");
                    return 2;
                }
                path = Some(a.as_str());
            }
        }
    }
    let Some(path) = path else {
        eprintln!("error: `twec verify` requires a file path");
        eprintln!("{USAGE}");
        return 2;
    };
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            // `twec verify` always emits a JSON document so an LLM
            // consumer doesn't have to special-case "the file
            // didn't exist." A read failure becomes a single
            // `lex-error`-shaped diagnostic at line 1.
            let report = crate::verify::VerifyReport {
                file: Some(path.to_string()),
                strict: false,
                verified: false,
                diagnostics: vec![crate::verify::VerifyDiagnostic {
                    kind: "io-error".to_string(),
                    severity: crate::verify::Severity::Error,
                    line: 1,
                    col: 1,
                    message: format!("cannot read '{path}': {e}"),
                    help: None,
                    fix: None,
                }],
            };
            println!("{}", report.to_json());
            return 1;
        }
    };
    let options = crate::verify::VerifyOptions { warn_deprecated };
    let report = crate::verify::verify_program_with_options(&source, Some(path), &options);
    println!("{}", report.to_json());
    if report.ok() {
        0
    } else {
        1
    }
}

/// Phase 33 session 1: `twec grammar [--format gbnf|json-schema|ebnf] [-o PATH]`.
/// Emits the canonical Twe grammar in the requested format. Default
/// format is GBNF (the highest-leverage target — llama.cpp constrained
/// decoding makes syntactic hallucination mechanically impossible).
/// Writes to stdout unless `-o` is given.
fn handle_grammar(args: &[String]) -> i32 {
    let mut format = crate::grammar::Format::Gbnf;
    let mut out_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --format takes an argument (gbnf|json-schema|ebnf)");
                    return 2;
                }
                let raw = args[i + 1].as_str();
                match crate::grammar::Format::parse(raw) {
                    Some(f) => format = f,
                    None => {
                        eprintln!("error: unknown grammar format `{raw}` (expected gbnf|json-schema|ebnf)");
                        return 2;
                    }
                }
                i += 2;
            }
            "-o" | "--out" => {
                if i + 1 >= args.len() {
                    eprintln!("error: -o takes a path argument");
                    return 2;
                }
                out_path = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                eprintln!("error: unknown argument for `grammar`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let body = crate::grammar::export(format);
    match out_path {
        Some(p) => match fs::write(&p, &body) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: cannot write `{p}`: {e}");
                1
            }
        },
        None => {
            print!("{body}");
            0
        }
    }
}

/// Phase 33 session 3: `twec stdlib [--json] [--category NAME] [-o PATH]`.
/// Emits the stdlib manifest. Default format is JSON (the only format
/// for now — a textual table form may follow). The manifest is the LLM's
/// grounding surface: every callable is listed with its category, params,
/// and (where available) doc string.
fn handle_stdlib(args: &[String]) -> i32 {
    let mut category: Option<String> = None;
    let mut out_path: Option<String> = None;
    // `--json` is currently the only supported format; accepted as a
    // no-op so future text-table support won't be a breaking change.
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                i += 1;
            }
            "--category" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --category takes a name argument");
                    return 2;
                }
                category = Some(args[i + 1].clone());
                i += 2;
            }
            "-o" | "--out" => {
                if i + 1 >= args.len() {
                    eprintln!("error: -o takes a path argument");
                    return 2;
                }
                out_path = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                eprintln!("error: unknown argument for `stdlib`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let manifest = crate::stdlib::manifest();
    let filtered: Vec<&crate::stdlib::BuiltinSpec> = match &category {
        Some(c) => manifest.iter().filter(|s| s.category == *c).collect(),
        None => manifest.iter().collect(),
    };
    let body = crate::stdlib::manifest_to_json(&filtered);
    match out_path {
        Some(p) => match fs::write(&p, &body) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: cannot write `{p}`: {e}");
                1
            }
        },
        None => {
            println!("{body}");
            0
        }
    }
}

/// Phase 33 session 4: `twec llm-loop`. web3d-M5: providers from
/// `twe-llm` — an API (`--provider anthropic|openai --model M`, with the
/// `llm-http` feature) or any command (`--command CMD [--arg A]*`,
/// which gets the prompt on stdin and prints the reply).
///
/// Drives the generate → verify → feed back loop on the task in
/// `--prompt` (or stdin), from `--starter` if given. The Twe primer is
/// the system prompt unless `--no-primer`. Each round is logged to the
/// trace directory (prompt, reply, verify JSON, tokens, cost).
fn handle_llm_loop(args: &[String]) -> i32 {
    let mut flags = ProviderFlags::default();
    let mut prompt_path: Option<String> = None;
    let mut starter_path: Option<String> = None;
    let mut max_rounds: u32 = 5;
    let mut out_path: Option<String> = None;
    let mut trace_dir: Option<String> = None;
    let mut primer = true;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag == "--no-primer" {
            primer = false;
            i += 1;
            continue;
        }
        let Some(value) = args.get(i + 1).cloned() else {
            eprintln!("error: {flag} takes a value (or is unknown to `llm-loop`)");
            eprintln!("{USAGE}");
            return 2;
        };
        match flag {
            "--prompt" => prompt_path = Some(value),
            "--starter" => starter_path = Some(value),
            "--out" | "-o" => out_path = Some(value),
            "--trace-dir" => trace_dir = Some(value),
            "--max-rounds" => match value.parse::<u32>() {
                Ok(n) if n >= 1 => max_rounds = n,
                _ => {
                    eprintln!("error: --max-rounds must be a positive integer");
                    return 2;
                }
            },
            _ => {
                if !flags.take(flag, value) {
                    eprintln!("error: unknown argument for `llm-loop`: {flag}");
                    eprintln!("{USAGE}");
                    return 2;
                }
            }
        }
        i += 2;
    }
    let mut provider = match flags.build() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let read = |path: &str| {
        fs::read_to_string(path).map_err(|e| {
            eprintln!("error: cannot read `{path}`: {e}");
            1
        })
    };
    let prompt = match prompt_path.as_deref() {
        Some(p) => match read(p) {
            Ok(s) => s,
            Err(code) => return code,
        },
        None => {
            // `cat task.md | twec llm-loop --command claude --arg -p`.
            use std::io::Read;
            let mut s = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut s) {
                eprintln!("error: reading prompt from stdin failed: {e}");
                return 1;
            }
            s
        }
    };
    let starter = match starter_path.as_deref() {
        Some(p) => match read(p) {
            Ok(s) => s,
            Err(code) => return code,
        },
        None => String::new(),
    };

    let options = crate::llm_loop::LoopOptions {
        max_rounds,
        trace_dir: trace_dir.map(std::path::PathBuf::from),
        source_path: out_path.clone(),
        log_prompts: true,
        system: if primer {
            crate::primer::guide().to_string()
        } else {
            String::new()
        },
        starter,
        ..Default::default()
    };
    let outcome = match crate::llm_loop::run_loop(provider.as_mut(), &prompt, &options) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: llm-loop failed: {e}");
            return 1;
        }
    };
    if let Some(p) = out_path.as_ref() {
        if let Err(e) = fs::write(p, &outcome.final_source) {
            eprintln!("error: cannot write `{p}`: {e}");
            return 1;
        }
    } else {
        print!("{}", outcome.final_source);
    }
    let u = outcome.usage;
    eprintln!(
        "[twec llm-loop] {} after {} round(s); tokens: {} in, {} out, {} cache read, {} cache write{}{}",
        if outcome.passed { "PASSED" } else { "FAILED" },
        outcome.rounds.len(),
        u.input,
        u.output,
        u.cache_read,
        u.cache_write,
        match outcome.cost_usd {
            Some(c) => format!("; ${c:.4}"),
            None => String::new(),
        },
        match outcome.trace_path.as_ref() {
            Some(p) => format!(" (trace: {})", p.display()),
            None => String::new(),
        }
    );
    if outcome.passed {
        0
    } else {
        1
    }
}

/// web3d-M5: `twec bench` — the LLM benchmark (`src/bench.rs`).
///
/// - `bench grade <task-dir> <file|-> [--json]` grades one program
///   (the benchmark runs this in a child process per program);
/// - `bench check [--all | <task-dir>...] [--jobs N] [--timeout S]`
///   validates tasks: the solution passes, the starter fails, every
///   check is caught by some running mutant of the solution.
fn handle_bench(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("grade") => bench_grade(&args[1..]),
        Some("check") => bench_check(&args[1..]),
        Some("run") => bench_run(&args[1..]),
        Some("regrade") => bench_regrade(&args[1..]),
        _ => {
            eprintln!("usage: twec bench grade <task-dir> <file|-> [--json] [--lang twe|python] [--python PATH]");
            eprintln!("       twec bench check [--all | <task-dir>...] [--jobs N] [--timeout SECONDS] [--lang twe|python]");
            eprintln!("       twec bench run <provider flags> [--lang twe|python] [--samples N] [--rounds N] [--no-verify] [--no-smoke] [--no-primer]");
            eprintln!("                      [--tasks a,b] [--jobs N] [--max-tokens N] [--timeout S] [--out DIR] [--cache DIR | --no-cache]");
            eprintln!("       twec bench regrade <run-dir>");
            2
        }
    }
}

/// web3d-M5 session 5: `--lang twe|python` and `--python PATH`, taken
/// out of a `bench` command's arguments.
struct LangFlags {
    rest: Vec<String>,
    lang: crate::bench::Lang,
    python: Option<std::path::PathBuf>,
    /// Where the tasks live (`--tasks-root`; default `bench/tasks`). A
    /// hidden task set is kept outside the repository and named here.
    tasks_root: std::path::PathBuf,
}

fn take_lang_flags(args: &[String]) -> Result<LangFlags, i32> {
    let mut out = LangFlags {
        rest: Vec::new(),
        lang: crate::bench::Lang::Twe,
        python: None,
        tasks_root: "bench/tasks".into(),
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lang" => {
                out.lang = match args.get(i + 1).map(String::as_str) {
                    Some("twe") => crate::bench::Lang::Twe,
                    Some("python") => crate::bench::Lang::Python,
                    _ => {
                        eprintln!("error: --lang takes `twe` or `python`");
                        return Err(2);
                    }
                };
                i += 2;
            }
            "--python" => {
                let Some(p) = args.get(i + 1) else {
                    eprintln!("error: --python takes the interpreter's path");
                    return Err(2);
                };
                out.python = Some(p.into());
                i += 2;
            }
            "--tasks-root" => {
                let Some(p) = args.get(i + 1) else {
                    eprintln!("error: --tasks-root takes a directory");
                    return Err(2);
                };
                out.tasks_root = p.into();
                i += 2;
            }
            _ => {
                out.rest.push(args[i].clone());
                i += 1;
            }
        }
    }
    Ok(out)
}

/// The grader for `lang`: this binary for Twe; for Python, the
/// interpreter (`--python`, else `bench/python/.venv`, else the path's)
/// and `bench/python/harness.py`.
fn make_grader(lang: crate::bench::Lang, python: Option<std::path::PathBuf>) -> Result<crate::bench::Grader, i32> {
    let exe = std::env::current_exe().map_err(|e| {
        eprintln!("error: cannot find twec itself: {e}");
        2
    })?;
    Ok(match lang {
        crate::bench::Lang::Twe => crate::bench::Grader::twe(&exe),
        crate::bench::Lang::Python => {
            let harness = std::path::Path::new(crate::bench::PYTHON_HARNESS);
            if !harness.exists() {
                eprintln!("error: {} not found (run from the repository root)", harness.display());
                return Err(2);
            }
            let python = python.unwrap_or_else(crate::bench::default_python);
            crate::bench::Grader::python(&exe, &python, harness)
        }
    })
}

fn bench_grade(args: &[String]) -> i32 {
    let LangFlags { rest: args, lang, python, .. } = match take_lang_flags(args) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let args = &args[..];
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "--json").collect();
    let [task_dir, file] = rest.as_slice() else {
        eprintln!("usage: twec bench grade <task-dir> <file|-> [--json]");
        return 2;
    };
    let task = match crate::bench::load_task(std::path::Path::new(task_dir.as_str())) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let source = if file.as_str() == "-" {
        use std::io::Read;
        let mut s = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut s) {
            eprintln!("error: reading the program from stdin failed: {e}");
            return 2;
        }
        s
    } else {
        match fs::read_to_string(file.as_str()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read `{file}`: {e}");
                return 2;
            }
        }
    };
    let grade = match lang {
        crate::bench::Lang::Twe => crate::bench::grade(&task, &source),
        crate::bench::Lang::Python => match make_grader(lang, python) {
            Ok(g) => g.grade(&task, &source, std::time::Duration::from_secs(60)),
            Err(code) => return code,
        },
    };
    if json {
        println!("{}", grade.to_json());
    } else {
        println!(
            "{}: {} ({}, {} of {} ticks)",
            task.id,
            if grade.passed { "PASS" } else { "FAIL" },
            grade.stage.as_str(),
            grade.ticks_run,
            task.ticks
        );
        if let Some(e) = &grade.error {
            println!("  error: {e}");
        }
        for c in &grade.checks {
            println!("  [{}] {} = {}", if c.passed { "x" } else { " " }, c.name, c.detail);
        }
    }
    if grade.passed {
        0
    } else {
        1
    }
}

/// web3d-M5 session 4: `twec bench run` — models write programs for
/// the tasks, which are graded on behaviour (`src/bench_run.rs`).
fn bench_run(args: &[String]) -> i32 {
    let LangFlags { rest: args, lang, python, tasks_root } = match take_lang_flags(args) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let args = &args[..];
    let grader = match make_grader(lang, python) {
        Ok(g) => g,
        Err(code) => return code,
    };
    let mut flags = ProviderFlags::default();
    let mut options = crate::bench_run::RunOptions {
        grader,
        jobs: 4,
        ..Default::default()
    };
    let mut only: Option<Vec<String>> = None;
    let mut out: Option<std::path::PathBuf> = None;
    let mut max_tokens: Option<u32> = None;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--no-verify" => options.verify_feedback = false,
            "--no-smoke" => options.smoke_feedback = false,
            "--no-primer" => options.primer = false,
            "--no-cache" => options.cache_dir = None,
            _ => {
                let Some(value) = args.get(i + 1).cloned() else {
                    eprintln!("error: {flag} takes a value (or is unknown to `bench run`)");
                    return 2;
                };
                let number = |v: &str| v.parse::<u32>().ok().filter(|n| *n >= 1);
                match flag {
                    "--samples" | "--rounds" | "--jobs" | "--max-tokens" | "--timeout" => {
                        let Some(n) = number(&value) else {
                            eprintln!("error: {flag} takes a positive whole number");
                            return 2;
                        };
                        match flag {
                            "--samples" => options.samples = n,
                            "--rounds" => options.max_rounds = n,
                            "--jobs" => options.jobs = n as usize,
                            "--max-tokens" => max_tokens = Some(n),
                            _ => options.grade_limit = std::time::Duration::from_secs(u64::from(n)),
                        }
                    }
                    "--tasks" => only = Some(value.split(',').map(|s| s.trim().to_string()).collect()),
                    "--out" => out = Some(value.into()),
                    "--cache" => options.cache_dir = Some(value.into()),
                    _ => {
                        if !flags.take(flag, value) {
                            eprintln!("error: unknown argument for `bench run`: {flag}");
                            return 2;
                        }
                    }
                }
                i += 1;
            }
        }
        i += 1;
    }
    if let Some(n) = max_tokens {
        options.max_tokens = n;
    }
    let provider_id = match flags.build() {
        Ok(p) => p.id(),
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let root = tasks_root.as_path();
    let mut dirs: Vec<std::path::PathBuf> = match fs::read_dir(root) {
        Ok(rd) => rd.flatten().map(|e| e.path()).filter(|p| p.join("task.toml").exists()).collect(),
        Err(e) => {
            eprintln!("error: {}: {e} (run from the repository root)", root.display());
            return 2;
        }
    };
    dirs.sort();
    if let Some(only) = &only {
        for name in only {
            if !dirs.iter().any(|d| d.file_name().is_some_and(|f| f == name.as_str())) {
                eprintln!("error: no task `{name}` in {}", root.display());
                return 2;
            }
        }
        dirs.retain(|d| only.iter().any(|n| d.file_name().is_some_and(|f| f == n.as_str())));
    }
    let mut tasks = Vec::new();
    for d in &dirs {
        match crate::bench::load_task(d) {
            Ok(t) => tasks.push(t),
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    }
    options.out_dir = out.unwrap_or_else(|| {
        let label: String = provider_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
            .collect();
        let lang = match options.grader.lang {
            crate::bench::Lang::Twe => String::new(),
            other => format!("{}-", other.as_str()),
        };
        std::path::PathBuf::from(format!("bench/runs/{}-{lang}{label}", crate::bench_run::today()))
    });
    eprintln!(
        "[bench] {} tasks × {} samples, {} with {} → {}",
        tasks.len(),
        options.samples,
        options.grader.lang.as_str(),
        provider_id,
        options.out_dir.display()
    );
    let make = || flags.build();
    match crate::bench_run::run(&tasks, &make, &provider_id, &options) {
        Ok(_) => {
            match fs::read_to_string(options.out_dir.join("summary.md")) {
                Ok(md) => print!("{md}"),
                Err(e) => eprintln!("error: reading the summary: {e}"),
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// `twec bench regrade <run-dir>`: grade a run's programs again with
/// the current grader and tasks (no model calls).
fn bench_regrade(args: &[String]) -> i32 {
    let LangFlags { rest, python, tasks_root, .. } = match take_lang_flags(args) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let [dir] = rest.as_slice() else {
        eprintln!("usage: twec bench regrade <run-dir> [--python PATH]");
        return 2;
    };
    // The run's language is in its run.json.
    let lang = match fs::read_to_string(std::path::Path::new(dir).join("run.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["lang"].as_str().map(str::to_string))
        .as_deref()
    {
        Some("python") => crate::bench::Lang::Python,
        _ => crate::bench::Lang::Twe,
    };
    let grader = match make_grader(lang, python) {
        Ok(g) => g,
        Err(code) => return code,
    };
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    match crate::bench_run::regrade(
        &grader,
        std::path::Path::new(dir),
        &tasks_root,
        std::time::Duration::from_secs(30),
        jobs,
    ) {
        Ok(_) => {
            if let Ok(md) = fs::read_to_string(std::path::Path::new(dir).join("summary.md")) {
                print!("{md}");
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn bench_check(args: &[String]) -> i32 {
    let LangFlags { rest: args, lang, python, tasks_root } = match take_lang_flags(args) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let args = &args[..];
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut timeout = 20.0_f64;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--all" => {
                let root = tasks_root.as_path();
                match fs::read_dir(root) {
                    Ok(rd) => {
                        let mut found: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.join("task.toml").exists()).collect();
                        found.sort();
                        dirs.extend(found);
                    }
                    Err(e) => {
                        eprintln!("error: {}: {e}", root.display());
                        return 2;
                    }
                }
            }
            "--jobs" | "--timeout" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("error: {} takes a number", args[i]);
                    return 2;
                };
                let ok = if args[i] == "--jobs" {
                    v.parse().map(|n: usize| jobs = n.max(1)).is_ok()
                } else {
                    v.parse().map(|s: f64| timeout = s).is_ok()
                };
                if !ok {
                    eprintln!("error: {} takes a number", args[i]);
                    return 2;
                }
                i += 1;
            }
            dir => dirs.push(dir.into()),
        }
        i += 1;
    }
    if dirs.is_empty() {
        eprintln!("usage: twec bench check [--all | <task-dir>...] [--jobs N] [--timeout SECONDS]");
        return 2;
    }
    let grader = match make_grader(lang, python) {
        Ok(g) => g,
        Err(code) => return code,
    };
    let limit = std::time::Duration::from_secs_f64(timeout);
    let mut bad = 0;
    for dir in &dirs {
        let task = match crate::bench::load_task(dir) {
            Ok(t) => t,
            Err(e) => {
                println!("{}: INVALID\n  {e}", dir.display());
                bad += 1;
                continue;
            }
        };
        match crate::bench::validate(&grader, &task, limit, jobs) {
            Ok(v) => {
                let strengths: Vec<String> = v.strengths.iter().map(|s| format!("{} ({})", s.name, s.killed)).collect();
                println!(
                    "{}: {} — {} checks, {} of {} mutants run; caught by: {}",
                    v.task,
                    if v.ok() { "ok" } else { "INVALID" },
                    task.checks.len(),
                    v.runnable,
                    v.mutants,
                    strengths.join(", ")
                );
                for p in &v.problems {
                    println!("  {p}");
                }
                if !v.ok() {
                    bad += 1;
                }
            }
            Err(e) => {
                println!("{}: INVALID\n  {e}", task.id);
                bad += 1;
            }
        }
    }
    println!("{} of {} tasks valid", dirs.len() - bad, dirs.len());
    if bad == 0 {
        0
    } else {
        1
    }
}

/// web3d-M5: the flags that choose a model, shared by `llm-loop` and
/// `bench`:
///
/// - `--provider anthropic --model M [--effort E]`
/// - `--provider openai --model M [--base-url URL]` (default: Ollama's)
/// - `--command CMD [--arg A]*` (or `--provider command`)
#[derive(Default, Clone)]
struct ProviderFlags {
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    base_url: Option<String>,
    command: Option<String>,
    args: Vec<String>,
    /// `--gbnf`: constrain an OpenAI-compatible (llama.cpp) server's
    /// decoding to Twe's grammar.
    gbnf: bool,
}

impl ProviderFlags {
    /// Take `flag` with its `value` if it's a provider flag.
    fn take(&mut self, flag: &str, value: String) -> bool {
        match flag {
            "--provider" => self.provider = Some(value),
            "--model" => self.model = Some(value),
            "--effort" => self.effort = Some(value),
            "--base-url" => self.base_url = Some(value),
            "--command" | "--cmd" => self.command = Some(value),
            "--arg" => self.args.push(value),
            "--gbnf" => self.gbnf = value == "on",
            _ => return false,
        }
        true
    }

    fn build(&self) -> Result<Box<dyn twe_llm::Provider>, String> {
        let kind = match (&self.provider, &self.command) {
            (Some(p), _) => p.clone(),
            (None, Some(_)) => "command".to_string(),
            (None, None) => {
                return Err(
                    "choose a model: --provider anthropic|openai --model M, or --command CMD".into(),
                )
            }
        };
        let model = || {
            self.model
                .clone()
                .ok_or_else(|| format!("--provider {kind} needs --model"))
        };
        match kind.as_str() {
            "anthropic" => {
                let mut p = twe_llm::anthropic::Anthropic::new(model()?);
                p.effort = self.effort.clone();
                Ok(Box::new(p))
            }
            "openai" => {
                let base = self
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434/v1".into());
                let mut p = twe_llm::openai::OpenAiCompatible::new(base, model()?);
                if self.gbnf {
                    p.grammar = Some(crate::grammar::export(crate::grammar::Format::Gbnf));
                }
                Ok(Box::new(p))
            }
            "command" => match &self.command {
                Some(c) => Ok(Box::new(twe_llm::CommandProvider::new(c.clone(), self.args.clone()))),
                None => Err("--provider command needs --command CMD".into()),
            },
            other => Err(format!("unknown provider `{other}` (anthropic, openai, command)")),
        }
    }
}

/// Phase 33 session 5: stub. The real handler is added when the
/// `mcp` module lands (next in this same commit).
fn handle_mcp(args: &[String]) -> i32 {
    if !args.is_empty() {
        eprintln!("error: `twec mcp` takes no arguments");
        return 2;
    }
    crate::mcp::serve_stdio()
}

/// Print the LLM grounding primer so out-of-process clients (Twe Studio's
/// in-app AI prompt) can fold it into their system prompt — the same text the
/// MCP server ships in its `instructions` field. `--full` prints the complete
/// guide (the `twe://guide` resource body) instead of the concise primer.
fn handle_primer(args: &[String]) -> i32 {
    let full = args.iter().any(|a| a == "--full");
    if let Some(unknown) = args.iter().find(|a| *a != "--full") {
        eprintln!("error: unknown argument '{unknown}' (usage: twec primer [--full])");
        return 2;
    }
    if full {
        println!("{}", crate::primer::guide());
    } else {
        println!("{}", crate::primer::INSTRUCTIONS);
    }
    0
}

/// Phase 33 session 6: emit the labeled examples corpus as JSON.
fn handle_corpus(args: &[String]) -> i32 {
    let mut out_path: Option<String> = None;
    let mut root: String = "examples".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                i += 1;
            }
            "--root" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --root takes a directory");
                    return 2;
                }
                root = args[i + 1].clone();
                i += 2;
            }
            "-o" | "--out" => {
                if i + 1 >= args.len() {
                    eprintln!("error: -o takes a path argument");
                    return 2;
                }
                out_path = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                eprintln!("error: unknown argument for `corpus`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let entries = crate::corpus::scan_corpus(std::path::Path::new(&root));
    let body = crate::corpus::to_json(&entries);
    match out_path {
        Some(p) => match fs::write(&p, &body) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: cannot write `{p}`: {e}");
                1
            }
        },
        None => {
            println!("{body}");
            0
        }
    }
}

/// Phase 33 session 7: `twec eval [SUITE] [--source FILE] [--source-dir DIR]
/// [--root DIR] [--json] [-o PATH]`. Grade one or all suites against
/// a generated `.twe` source. Without `--source`/`--source-dir` lists
/// available suites and exits 0 (the no-LLM dry-run mode).
fn handle_eval(args: &[String]) -> i32 {
    // web3d-M5: superseded by `twec bench` (behavioural grading).
    eprintln!("note: `twec eval` is deprecated; use `twec bench` (bench/README.md), which grades programs by behaviour");
    let mut suite_name: Option<String> = None;
    let mut source_file: Option<String> = None;
    let mut source_dir: Option<String> = None;
    let mut root: String = "eval".to_string();
    let mut out_path: Option<String> = None;
    // `--json` is the default and only output today; accept the flag
    // as a no-op so future text output is not a breaking change.
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --source takes a path argument");
                    return 2;
                }
                source_file = Some(args[i + 1].clone());
                i += 2;
            }
            "--source-dir" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --source-dir takes a directory");
                    return 2;
                }
                source_dir = Some(args[i + 1].clone());
                i += 2;
            }
            "--root" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --root takes a directory");
                    return 2;
                }
                root = args[i + 1].clone();
                i += 2;
            }
            "-o" | "--out" => {
                if i + 1 >= args.len() {
                    eprintln!("error: -o takes a path argument");
                    return 2;
                }
                out_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--json" => {
                i += 1;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown flag for `eval`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
            other => {
                if suite_name.is_some() {
                    eprintln!("error: `eval` takes at most one positional suite name");
                    return 2;
                }
                suite_name = Some(other.to_string());
                i += 1;
            }
        }
    }

    // Discover suites.
    let root_path = std::path::PathBuf::from(&root);
    let suites = match discover_suites(&root_path, suite_name.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    if suites.is_empty() {
        eprintln!("no suites found under `{root}`");
        return 1;
    }

    // Score each suite. For a single source file with multiple suites,
    // grade the same source against each (useful for "did the model
    // hit at least one target"). For `--source-dir`, look for one
    // file named `<suite>.twe` inside the dir per suite.
    let mut scores: Vec<crate::llm_eval::Score> = Vec::new();
    for suite in &suites {
        let source_text = if let Some(p) = source_file.as_ref() {
            match fs::read_to_string(p) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: cannot read --source `{p}`: {e}");
                    return 1;
                }
            }
        } else if let Some(dir) = source_dir.as_ref() {
            let candidate = std::path::Path::new(dir).join(format!("{}.twe", suite.name));
            match fs::read_to_string(&candidate) {
                Ok(s) => s,
                Err(_) => {
                    // Missing file = not generated yet = skipped, not error.
                    continue;
                }
            }
        } else {
            // Dry run: no source. Just list the suite.
            continue;
        };
        scores.push(crate::llm_eval::grade_source(suite, &source_text));
    }

    if scores.is_empty() {
        // List discovered suites so the user knows what's available.
        let mut listing = String::from("{\"tool\":\"twec-eval\",\"version\":1,\"suites\":[");
        for (i, s) in suites.iter().enumerate() {
            if i > 0 {
                listing.push(',');
            }
            listing.push('"');
            listing.push_str(&s.name);
            listing.push('"');
        }
        listing.push_str("]}");
        match out_path {
            Some(p) => {
                if let Err(e) = fs::write(&p, listing) {
                    eprintln!("error: cannot write `{p}`: {e}");
                    return 1;
                }
            }
            None => println!("{listing}"),
        }
        return 0;
    }

    let body = crate::llm_eval::scorecard_json(&scores);
    let any_failed = scores.iter().any(|s| !s.passed);
    match out_path {
        Some(p) => {
            if let Err(e) = fs::write(&p, &body) {
                eprintln!("error: cannot write `{p}`: {e}");
                return 1;
            }
        }
        None => println!("{body}"),
    }
    if any_failed {
        1
    } else {
        0
    }
}

fn discover_suites(
    root: &std::path::Path,
    only: Option<&str>,
) -> Result<Vec<crate::llm_eval::Suite>, String> {
    if !root.is_dir() {
        return Err(format!("eval root `{}` is not a directory", root.display()));
    }
    let entries = fs::read_dir(root).map_err(|e| format!("read_dir `{}`: {e}", root.display()))?;
    let mut suites = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if let Some(name_filter) = only {
            if p.file_name().and_then(|s| s.to_str()) != Some(name_filter) {
                continue;
            }
        }
        // Skip directories that don't actually carry a suite (no
        // expected.txt). Lets `eval/` host docs / scratch dirs.
        if !p.join("expected.txt").exists() {
            continue;
        }
        match crate::llm_eval::load_suite(&p) {
            Ok(s) => suites.push(s),
            Err(e) => return Err(e),
        }
    }
    suites.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(suites)
}

/// Phase 33 session 8: `twec mutate [--root DIR] [--out DIR] [--rules RULESET]`.
/// Walk every `.twe` file under `--root` (default `tests/programs`),
/// apply each enabled mutation rule, capture (broken, verify_json,
/// fix_json) triples, and write them as JSONL into `--out` (default
/// `corpus/error_fix/`). The output is the fine-tune training set.
fn handle_mutate(args: &[String]) -> i32 {
    let mut root: String = "tests/programs".to_string();
    let mut out: String = "corpus/error_fix".to_string();
    let mut rules: String = "all".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --root takes a directory");
                    return 2;
                }
                root = args[i + 1].clone();
                i += 2;
            }
            "--out" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --out takes a directory");
                    return 2;
                }
                out = args[i + 1].clone();
                i += 2;
            }
            "--rules" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --rules takes a name");
                    return 2;
                }
                rules = args[i + 1].clone();
                i += 2;
            }
            other => {
                eprintln!("error: unknown argument for `mutate`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let root_p = std::path::PathBuf::from(&root);
    let out_p = std::path::PathBuf::from(&out);
    if let Err(e) = fs::create_dir_all(&out_p) {
        eprintln!("error: cannot create `{}`: {e}", out_p.display());
        return 1;
    }
    let report = crate::mutator::run(&root_p, &out_p, crate::mutator::RuleSet::parse(&rules));
    println!("{}", report.summary());
    if report.triples_emitted == 0 {
        1
    } else {
        0
    }
}

/// Phase 35 session 1: `twec api-snapshot [-o PATH]`. Writes a
/// canonical, hashable JSON document of every public-API surface
/// (stdlib manifest + keywords + tool versions) to PATH or stdout.
/// Suggested checkin location for routine snapshots:
/// `docs/api-snapshots/<YYYY-MM-DD>.json`.
fn handle_api_snapshot(args: &[String]) -> i32 {
    let mut out_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                if i + 1 >= args.len() {
                    eprintln!("error: -o takes a path argument");
                    return 2;
                }
                out_path = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                eprintln!("error: unknown argument for `api-snapshot`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let body = crate::api_snapshot::snapshot_json();
    match out_path {
        Some(p) => match fs::write(&p, &body) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: cannot write `{p}`: {e}");
                1
            }
        },
        None => {
            print!("{body}");
            0
        }
    }
}

/// Phase 35 session 1: `twec api-diff <old> <new>`. Reads two
/// snapshot files and reports any drift (builtins added/removed,
/// builtins with changed signatures, keyword changes, tool-version
/// bumps). Exits 0 if identical, 3 if drift was detected — suitable
/// for `cargo make api-stability-gate` style CI use.
fn handle_api_diff(args: &[String]) -> i32 {
    if args.len() != 2 {
        eprintln!("error: `twec api-diff` takes two snapshot paths");
        eprintln!("usage: twec api-diff <old.json> <new.json>");
        return 2;
    }
    let old = match fs::read_to_string(&args[0]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read `{}`: {e}", args[0]);
            return 1;
        }
    };
    let new = match fs::read_to_string(&args[1]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read `{}`: {e}", args[1]);
            return 1;
        }
    };
    let d = crate::api_snapshot::diff(&old, &new);
    if d.is_clean() {
        println!("api-diff: clean (every public surface matched)");
        return 0;
    }
    if !d.builtins_added.is_empty() {
        println!("builtins added ({}):", d.builtins_added.len());
        for n in &d.builtins_added {
            println!("  + {n}");
        }
    }
    if !d.builtins_removed.is_empty() {
        println!("builtins removed ({}):", d.builtins_removed.len());
        for n in &d.builtins_removed {
            println!("  - {n}");
        }
    }
    if !d.builtins_changed.is_empty() {
        println!(
            "builtins with changed signatures ({}):",
            d.builtins_changed.len()
        );
        for n in &d.builtins_changed {
            println!("  ~ {n}");
        }
    }
    if !d.keywords_added.is_empty() {
        println!("keywords added ({}):", d.keywords_added.len());
        for k in &d.keywords_added {
            println!("  + {k}");
        }
    }
    if !d.keywords_removed.is_empty() {
        println!("keywords removed ({}):", d.keywords_removed.len());
        for k in &d.keywords_removed {
            println!("  - {k}");
        }
    }
    if !d.tool_version_changes.is_empty() {
        println!("tool version changes ({}):", d.tool_version_changes.len());
        for (name, old_v, new_v) in &d.tool_version_changes {
            println!("  ~ {name}: v{old_v} -> v{new_v}");
        }
    }
    3
}

/// v1.0.1 session 11: `twec perf-snapshot [--target DIR] [-o PATH]`.
/// Scrapes `<target-dir>/criterion/` for every bench's
/// `new/estimates.json` (falls back to `base/`) and writes the
/// canonical JSON snapshot. Default target dir is `./target`;
/// default output is stdout.
fn handle_perf_snapshot(args: &[String]) -> i32 {
    let mut target: std::path::PathBuf = "target".into();
    let mut out: Option<std::path::PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("error: `--target` needs a value");
                    return 2;
                };
                target = v.into();
                i += 1;
            }
            "-o" | "--out" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("error: `-o` needs a path");
                    return 2;
                };
                out = Some(v.into());
                i += 1;
            }
            other => {
                eprintln!("error: unknown argument for `perf-snapshot`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let snap = match crate::perf_snapshot::scrape_criterion(&target) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let body = snap.to_json();
    match out {
        Some(p) => match crate::perf_snapshot::write_snapshot(&snap, &p) {
            Ok(()) => {
                eprintln!(
                    "[twec perf-snapshot] wrote {} ({} benches)",
                    p.display(),
                    snap.benches.len()
                );
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        None => {
            println!("{body}");
            0
        }
    }
}

/// v1.0.1 session 11: `twec perf-diff [--threshold PCT] <baseline.json>
/// <current.json>`. Exits 0 when no bench regressed beyond the
/// threshold (default 5%), 1 when a regression is found, 2 on usage
/// errors. The human-readable report goes to stdout in both cases.
fn handle_perf_diff(args: &[String]) -> i32 {
    let mut threshold = crate::perf_snapshot::DEFAULT_THRESHOLD_PCT;
    let mut positional: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--threshold" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("error: `--threshold` needs a percent value");
                    return 2;
                };
                match v.parse::<f64>() {
                    Ok(t) if t >= 0.0 => threshold = t,
                    Ok(_) => {
                        eprintln!("error: `--threshold` must be non-negative");
                        return 2;
                    }
                    Err(_) => {
                        eprintln!("error: `--threshold` must be a number");
                        return 2;
                    }
                }
                i += 1;
            }
            a if a.starts_with('-') => {
                eprintln!("error: unknown flag for `perf-diff`: {a}");
                eprintln!("{USAGE}");
                return 2;
            }
            _ => {
                positional.push(args[i].as_str());
                i += 1;
            }
        }
    }
    if positional.len() != 2 {
        eprintln!("error: `twec perf-diff` takes two snapshot paths");
        eprintln!("usage: twec perf-diff [--threshold PCT] <baseline.json> <current.json>");
        return 2;
    }
    let baseline = match crate::perf_snapshot::read_snapshot(std::path::Path::new(positional[0])) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let current = match crate::perf_snapshot::read_snapshot(std::path::Path::new(positional[1])) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let d = crate::perf_snapshot::diff(&baseline, &current);
    print!("{}", d.format_human(threshold));
    if d.regressed(threshold) {
        eprintln!(
            "perf-diff: at least one bench regressed beyond +{:.1}% — failing CI",
            threshold
        );
        1
    } else {
        0
    }
}

/// v1.0.1 session 13: `twec doctor [--json] [-o PATH]` — single-page
/// triage report. Prints human-readable text by default; `--json`
/// switches to the machine-readable schema documented in
/// `crate::doctor::Report::to_json`. Always exits 0 — the diagnostic
/// surfaces problems through `warnings`, never by failing the
/// process (a triage tool that exits non-zero is useless for the
/// "paste the output of `twec doctor`" support workflow).
fn handle_doctor(args: &[String]) -> i32 {
    let mut json = false;
    let mut out_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                json = true;
                i += 1;
            }
            "-o" | "--out" => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("error: `-o` takes a path argument");
                    return 2;
                };
                out_path = Some(v.clone());
                i += 1;
            }
            other => {
                eprintln!("error: unknown argument for `doctor`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let report = crate::doctor::Report::capture();
    let body = if json {
        report.to_json()
    } else {
        report.to_text()
    };
    match out_path {
        Some(p) => match fs::write(&p, &body) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: cannot write `{p}`: {e}");
                1
            }
        },
        None => {
            print!("{body}");
            if !body.ends_with('\n') {
                println!();
            }
            0
        }
    }
}

/// `twec play_visual <file>` — Phase 9 session 11: render the
/// first `visual` block in the file as a fullscreen wgpu fragment
/// shader. Time uniform is driven from the system clock; Esc
/// closes the window. Hot reload picks up edits to the source.
fn handle_play_visual(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("error: `twec play_visual` requires a file path");
        eprintln!("{USAGE}");
        return 2;
    }
    let mut path: Option<String> = None;
    for a in args {
        if a.starts_with('-') {
            eprintln!("error: unknown flag for `play_visual`: {a}");
            eprintln!("{USAGE}");
            return 2;
        }
        if path.is_some() {
            eprintln!("error: `twec play_visual` takes one file path");
            return 2;
        }
        path = Some(a.clone());
    }
    let path = path.expect("non-empty args + no flags ⇒ at least one positional");
    crate::play_visual::launch(path)
}

/// `twec fmt [--in-place|--check] <file>` — print the canonical
/// form of a Twe file. Default is to write to stdout.
/// `--in-place` overwrites the file. `--check` exits 0 if the
/// file is already in canonical form, 1 otherwise (no output) —
/// suitable for CI / pre-commit gating.
fn handle_fmt(args: &[String]) -> i32 {
    let mut in_place = false;
    let mut check = false;
    let mut path: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--in-place" | "-i" => {
                in_place = true;
                i += 1;
            }
            "--check" => {
                check = true;
                i += 1;
            }
            other if other.starts_with("--") => {
                eprintln!("error: unknown flag '{other}'");
                eprintln!("{USAGE}");
                return 2;
            }
            other => {
                if path.is_some() {
                    eprintln!("error: `twec fmt` takes a single file path");
                    return 2;
                }
                path = Some(other);
                i += 1;
            }
        }
    }
    if in_place && check {
        eprintln!("error: --in-place and --check are mutually exclusive");
        return 2;
    }
    let Some(path) = path else {
        eprintln!("error: `twec fmt` requires a file path");
        eprintln!("{USAGE}");
        return 2;
    };
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return 2;
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    // Phase 27: trivia-preserving fmt. Re-emits the source's
    // comments + blank lines at their original positions instead
    // of dropping them.
    let formatted = crate::printer::print_program_with_trivia(&program, &src);
    if check {
        if src == formatted {
            0
        } else {
            // Stay quiet; the exit code is the signal. CI scripts
            // can re-run without --check to see the diff.
            1
        }
    } else if in_place {
        if let Err(e) = fs::write(path, &formatted) {
            eprintln!("error: could not write '{path}': {e}");
            return 2;
        }
        0
    } else {
        print!("{formatted}");
        0
    }
}

/// `twec lsp` — speak Language Server Protocol over stdio.
/// Editor extensions launch the binary with this subcommand and
/// pipe LSP messages on stdin / read responses on stdout.
fn handle_lsp(args: &[String]) -> i32 {
    if !args.is_empty() {
        eprintln!("error: `twec lsp` takes no arguments");
        eprintln!("{USAGE}");
        return 2;
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match crate::lsp::run(stdin.lock(), stdout.lock()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("twec lsp: {e}");
            1
        }
    }
}

/// `twec types <file>` — print the inferred type of every
/// top-level binding in the file. Phase 4a literal-driven
/// inference: scalars, tuples, lists, ranges, comparisons,
/// arithmetic with int/float promotion, function arity,
/// class declarations. Names whose RHS we can't prove anything
/// about print as `?` (the lattice bottom — non-strict's
/// "no false positives" stance).
fn handle_types(args: &[String]) -> i32 {
    if args.len() != 1 {
        eprintln!("error: `twec types` takes a single file path");
        eprintln!("{USAGE}");
        return 2;
    }
    let path = &args[0];
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return 2;
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    // Strict mode is opted into by a `# strict` (or `#! strict`)
    // line in the first ten lines of the source. Without the
    // directive, behaviour matches v0.1 (silently absorb
    // unification failures); with it, the inferer accumulates
    // diagnostics that we surface here and the exit code goes
    // non-zero. Phase 6 session 1.
    let strict = crate::infer::detect_strict(&src);
    let (bindings, errors) = crate::infer::infer_program_strict(&program, strict);
    // Sort by name for deterministic output (handy for snapshot
    // testing + diffing across runs).
    let mut entries: Vec<(&String, &crate::types::Type)> = bindings.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, ty) in entries {
        println!("{name}: {ty}");
    }
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("{path}:{}:{}: type error: {}", e.line, e.col, e.message);
            if let Some(help) = &e.help {
                eprintln!("  help: {help}");
            }
        }
        // Exit non-zero so CI / pre-commit hooks gate strict files
        // on success. Non-strict files never reach this branch.
        return 1;
    }
    0
}

fn handle_parse(args: &[String]) -> i32 {
    if args.len() != 1 {
        eprintln!("error: `twec parse` takes a single file path");
        eprintln!("{USAGE}");
        return 2;
    }
    let path = &args[0];
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return 2;
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    println!("{}", crate::ast_json::to_json(&program));
    0
}

fn print_version() {
    println!("twec {}", env!("CARGO_PKG_VERSION"));
}

struct CommonFlags {
    frames: u32,
    path: Option<String>,
}

/// Shared --vm / --frames / positional-path parser. `allow_frames`
/// gates the `--frames N` flag (only `run` accepts it; `play` drives
/// frames from the macroquad clock).
fn parse_common_flags(args: &[String], allow_frames: bool) -> Result<CommonFlags, i32> {
    let mut frames: u32 = 0;
    let mut path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--vm" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --vm requires a value (only `tree` remains)");
                    return Err(2);
                }
                match args[i + 1].as_str() {
                    "tree" | "walker" => {}
                    "bytecode" | "bc" | "vm" => {
                        eprintln!("{VM_REMOVED}");
                        return Err(2);
                    }
                    other => {
                        eprintln!("error: --vm value must be 'tree', got '{other}'");
                        return Err(2);
                    }
                }
                i += 2;
            }
            "--frames" => {
                if !allow_frames {
                    eprintln!("error: --frames is only valid for `twec run`");
                    return Err(2);
                }
                if i + 1 >= args.len() {
                    eprintln!("error: --frames requires a number");
                    return Err(2);
                }
                frames = match args[i + 1].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("error: --frames value must be a non-negative integer");
                        return Err(2);
                    }
                };
                i += 2;
            }
            other if other.starts_with("--") => {
                eprintln!("error: unknown flag '{other}'");
                eprintln!("{USAGE}");
                return Err(2);
            }
            other => {
                if path.is_some() {
                    eprintln!("error: only one file path is allowed");
                    return Err(2);
                }
                path = Some(other.to_string());
                i += 1;
            }
        }
    }
    Ok(CommonFlags { frames, path })
}

/// `twec profile [--frames N] [-o trace.json] <file>` — run the
/// script through the tree-walker for `N` frames with profiling
/// enabled, then dump a Chrome Tracing JSON file. Defaults: 60
/// frames at 1/60s dt, output `<file>.trace.json` next to the source.
/// The bytecode VM doesn't ship instrumentation in this session
/// because the dispatch loop is hot enough that adding a per-call
/// probe would skew the very numbers Phase 11 session 7 is trying
/// to drive down. Profiling the tree-walker is enough to pressure-
/// test the trace format end-to-end.
fn handle_profile(args: &[String]) -> i32 {
    let mut frames: u32 = 60;
    let mut output: Option<String> = None;
    let mut path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frames" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --frames needs a value");
                    return 2;
                }
                match args[i + 1].parse::<u32>() {
                    Ok(n) => frames = n,
                    Err(_) => {
                        eprintln!("error: --frames takes a non-negative integer");
                        return 2;
                    }
                }
                i += 2;
            }
            "-o" | "--output" => {
                if i + 1 >= args.len() {
                    eprintln!("error: -o needs a value");
                    return 2;
                }
                output = Some(args[i + 1].clone());
                i += 2;
            }
            a if a.starts_with('-') => {
                eprintln!("error: unknown flag for `profile`: {a}");
                eprintln!("{USAGE}");
                return 2;
            }
            _ => {
                if path.is_some() {
                    eprintln!("error: `twec profile` takes one file path");
                    return 2;
                }
                path = Some(args[i].clone());
                i += 1;
            }
        }
    }
    let Some(path) = path else {
        eprintln!("error: `twec profile` requires a file path");
        eprintln!("{USAGE}");
        return 2;
    };
    let trace_path = output.unwrap_or_else(|| format!("{path}.trace.json"));

    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return 2;
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };

    crate::profile::enable();
    let result = crate::eval::run_with_frames(&program, frames, 1.0 / 60.0);
    crate::profile::disable();

    match result {
        Ok(_out) => {
            // Drop the script's print output during profiling — the
            // user is opting in to a trace, not script output. (If
            // they want both, they can run `twec run` separately.)
        }
        Err(e) => {
            eprintln!("{path}: runtime error: {e}");
            return 1;
        }
    }

    if let Err(e) = crate::profile::dump_to_path(std::path::Path::new(&trace_path)) {
        eprintln!("error: {e}");
        return 1;
    }
    eprintln!("[twec] trace written: {trace_path}");
    0
}

/// web3d-M1: register the script's directory as the asset root, so a
/// project-relative `load("assets/hero.png")` resolves no matter which
/// directory `twec` was launched from (see `bundle::set_asset_root`).
/// The script is the first argument naming an existing `.twe` file or
/// project directory.
fn register_asset_root(args: &[String]) {
    for a in args {
        let p = std::path::Path::new(a);
        let dir = if p.is_dir() {
            Some(p.to_path_buf())
        } else if p.is_file() && p.extension().is_some_and(|e| e == "twe") {
            p.parent().map(|d| d.to_path_buf())
        } else {
            None
        };
        if let Some(d) = dir {
            crate::bundle::set_asset_root(Some(d));
            return;
        }
    }
}

fn handle_run(args: &[String]) -> i32 {
    let parsed = match parse_common_flags(args, true) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let Some(path) = parsed.path else {
        eprintln!("error: `twec run` requires a file path");
        eprintln!("{USAGE}");
        return 2;
    };
    // v1.0.2 Session 6: `twec run <dir>` auto-detects `main.twe` and
    // routes through the module loader so multi-file projects work
    // out of the box. Closes the Phase 13 closeout deferral.
    if std::path::Path::new(&path).is_dir() {
        return run_project_dir(&path, parsed.frames);
    }
    run_file_tree(&path, parsed.frames)
}

/// v1.0.2 Session 6: run a multi-file project from a directory by
/// resolving `<dir>/main.twe` as the entry point and loading every
/// imported module via `crate::module`.
fn run_project_dir(dir: &str, frames: u32) -> i32 {
    let entry = std::path::Path::new(dir).join("main.twe");
    if !entry.exists() {
        eprintln!("error: `{dir}/main.twe` not found");
        eprintln!(
            "  help: `twec run <dir>` expects a `main.twe` at the project root; pass a file path directly if your entry has a different name"
        );
        return 2;
    }
    // web3d-M1: same path as running the file — modules load, frames tick.
    run_file_tree(&entry.to_string_lossy(), frames)
}

fn run_file_tree(path: &str, frames: u32) -> i32 {
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return 2;
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return 1;
        }
    };
    // web3d-M1: programs that `import` go through the module loader —
    // before, only `twec run <dir>` loaded modules (without ticking
    // frames), so an imported module was silently unbound here.
    let result = if crate::module::has_imports(&program) {
        match crate::module::prepare_entry(std::path::Path::new(path), &src) {
            Ok(mut env) => (|| {
                crate::eval::headless(&mut env);
                for _ in 0..frames {
                    crate::eval::tick_frame(&mut env, 1.0 / 60.0)?;
                    if env.returning.take().is_some() {
                        break;
                    }
                }
                Ok(env.out)
            })(),
            Err(msg) => {
                eprintln!("{msg}");
                return 1;
            }
        }
    } else if frames > 0 {
        crate::eval::run_with_frames(&program, frames, 1.0 / 60.0)
    } else {
        crate::eval::run(&program)
    };
    match result {
        Ok(out) => {
            print!("{out}");
            0
        }
        Err(e) => {
            eprintln!("{path}: runtime error: {e}");
            1
        }
    }
}

/// Phase 11 session 3: crash reporter. Replace the default panic
/// printer with one that:
///
/// 1. Prints a readable user-facing banner pointing at the dump
///    file, instead of dumping a Rust backtrace at the user.
/// 2. Writes a developer-readable bundle to the current directory:
///    timestamp, panic message + location, twec version, OS, and
///    a backtrace (when `RUST_BACKTRACE=1` is set or
///    `force_capture` succeeds in the current toolchain).
///
/// Set `TWEC_NO_CRASH_REPORTER=1` to bypass the hook (useful when
/// running under a debugger that wants to catch the panic itself).
/// The default Rust panic-hook output stays available too — we
/// invoke it after writing the dump, so terminal users still see
/// the colored Rust panic banner.
pub fn install_crash_reporter() {
    if std::env::var_os("TWEC_NO_CRASH_REPORTER").is_some() {
        return;
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = write_crash_dump(info);
        match path {
            Some(p) => eprintln!("\n[twec] crashed — dump written to {p}"),
            None => eprintln!("\n[twec] crashed — failed to write dump file"),
        }
        // Still print the standard Rust panic line so the developer
        // sees the message + location without opening the dump.
        default_hook(info);
    }));
}

fn write_crash_dump(info: &std::panic::PanicHookInfo<'_>) -> Option<String> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = std::env::var_os("TWEC_CRASH_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let pid = std::process::id();
    let path = dir.join(format!("twec-crash-{secs}-{pid}.log"));
    let body = format_crash_body(info, secs);
    fs::write(&path, body).ok()?;
    // v1.0.1 session 10: flush the input ring next to the .log so
    // the user (or `twec replay`) can reproduce the crash. Errors
    // are swallowed — we still want the .log even if the .replay
    // can't land.
    let replay_path = dir.join(format!("twec-crash-{secs}-{pid}.replay"));
    let _ = crate::replay::dump_ring_to(&replay_path);
    Some(path.display().to_string())
}

fn format_crash_body(info: &std::panic::PanicHookInfo<'_>, secs: u64) -> String {
    let msg = panic_payload_string(info);
    let loc = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown location>".to_string());
    let bt = std::backtrace::Backtrace::force_capture();
    format!(
        "twec crash report\n\
         =================\n\
         twec version: {ver}\n\
         os: {os}\n\
         arch: {arch}\n\
         unix-time: {secs}\n\
         \n\
         panic: {msg}\n\
         at: {loc}\n\
         \n\
         backtrace:\n\
         {bt}\n",
        ver = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    )
}

fn panic_payload_string(info: &std::panic::PanicHookInfo<'_>) -> String {
    let p = info.payload();
    if let Some(s) = p.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = p.downcast_ref::<String>() {
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}

#[cfg(test)]
mod crash_tests {
    use super::*;

    /// End-to-end smoke test: install the hook, trigger a panic via
    /// `catch_unwind`, confirm that a `twec-crash-*.log` file landed
    /// in the temp-dir override path and contains the expected
    /// fields. Uses `TWEC_CRASH_DIR` instead of mutating cwd so the
    /// test doesn't race with other suites that read relative paths.
    #[test]
    fn install_crash_reporter_writes_dump_on_panic() {
        let dir = std::env::temp_dir().join(format!("twec_crash_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // Set TWEC_CRASH_DIR before installing the hook; the writer
        // reads it on every invocation.
        // SAFETY-ish: cargo test runs threads in the same process,
        // but no other test reads TWEC_CRASH_DIR — we set + leave it
        // for the duration of this test. Removing at the end keeps
        // suites that may be added later from inheriting it.
        std::env::set_var("TWEC_CRASH_DIR", &dir);

        let saved = std::panic::take_hook();
        install_crash_reporter();

        let _ = std::panic::catch_unwind(|| {
            panic!("synthetic crash for the dump test");
        });

        std::panic::set_hook(saved);
        std::env::remove_var("TWEC_CRASH_DIR");

        let mut found: Option<std::path::PathBuf> = None;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("twec-crash-") && name.ends_with(".log") {
                    found = Some(e.path());
                    break;
                }
            }
        }

        let path = found.expect("dump file was not written");
        let body = std::fs::read_to_string(&path).expect("read dump");
        assert!(body.contains("twec version"), "missing version: {body}");
        assert!(
            body.contains("synthetic crash for the dump test"),
            "missing panic message: {body}"
        );
        assert!(body.contains("backtrace:"), "missing backtrace section");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
