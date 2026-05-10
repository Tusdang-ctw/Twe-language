use std::env;
use std::fs;
use std::process;

const USAGE: &str = "usage: twec [run [--vm tree|bytecode] [--frames N] <file> | \
     play [--vm tree|bytecode] <file> | \
     play3d <file> | \
     play_visual <file> | \
     profile [--frames N] [-o trace.json] <file> | \
     build [--target T] [--config C] [--out PATH] [--dry-run] [--steam] <project_dir> | \
     bundle [-o PATH] <project_dir> | \
     info <bundle-or-exe> | \
     verify [--warn-deprecated] <file> | \
     grammar [--format gbnf|json-schema|ebnf] [-o PATH] | \
     stdlib [--json] [--category NAME] [-o PATH] | \
     llm-loop --command CMD [--arg ARG]* [--prompt PATH] [--max-rounds N] [--out PATH] [--trace-dir DIR] | \
     mcp | \
     corpus [--json] [-o PATH] | \
     fmt [--in-place|--check] <file> | \
     types <file> | lsp | parse <file> | version]";

/// Which interpreter the CLI dispatches to. The tree-walker is the
/// default for backwards compatibility — it's been the production
/// interpreter since Phase 1 and runs every example end-to-end.
/// The bytecode VM (sessions 5–13) is opt-in via `--vm bytecode`
/// while it earns equivalent confidence in the wild. Tests cross-
/// check the two on every meaningful program.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Tree,
    Bytecode,
}

impl Backend {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "tree" | "walker" => Some(Backend::Tree),
            "bytecode" | "bc" | "vm" => Some(Backend::Bytecode),
            _ => None,
        }
    }
}

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
        // Phase 33 session 5: stdio JSON-RPC MCP server. Every Twe
        // tool becomes available to any MCP client (Claude Desktop,
        // Cursor, the future Twe Studio) with no bespoke wiring.
        "mcp" => process::exit(handle_mcp(&args[2..])),
        // Phase 33 session 6: enumerate the labeled examples corpus
        // built from `@task / @inputs / @expected / @category` headers.
        "corpus" => process::exit(handle_corpus(&args[2..])),
        "play3d" => process::exit(handle_play3d(&args[2..])),
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
    let code = match parsed.backend {
        Backend::Tree => crate::play::launch(path),
        Backend::Bytecode => crate::play::launch_bytecode(path),
    };
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
                        "error: unknown target '{v}' (try windows-x86_64, macos-aarch64, macos-x86_64, linux-x86_64)"
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

/// Phase 33 session 4: `twec llm-loop --command CMD [--arg ARG]*
/// [--prompt PATH] [--max-rounds N] [--out PATH] [--trace-dir DIR]`.
///
/// Drives an LLM authoring loop using a user-configured command
/// provider. The command receives the prompt on stdin and returns
/// the model's reply on stdout — point it at `claude code`, a
/// Python wrapper, a local `llama-cli`, or anything that fits the
/// pipe. Each round's prompt + reply + verify JSON is logged to the
/// trace directory for fine-tune corpus harvesting.
fn handle_llm_loop(args: &[String]) -> i32 {
    let mut command: Option<String> = None;
    let mut cmd_args: Vec<String> = Vec::new();
    let mut prompt_path: Option<String> = None;
    let mut max_rounds: u32 = 5;
    let mut out_path: Option<String> = None;
    let mut trace_dir: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--command" | "--cmd" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --command takes an argument");
                    return 2;
                }
                command = Some(args[i + 1].clone());
                i += 2;
            }
            "--arg" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --arg takes an argument");
                    return 2;
                }
                cmd_args.push(args[i + 1].clone());
                i += 2;
            }
            "--prompt" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --prompt takes a file path");
                    return 2;
                }
                prompt_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--max-rounds" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --max-rounds takes an integer");
                    return 2;
                }
                match args[i + 1].parse::<u32>() {
                    Ok(n) if n >= 1 => max_rounds = n,
                    _ => {
                        eprintln!("error: --max-rounds must be a positive integer");
                        return 2;
                    }
                }
                i += 2;
            }
            "--out" | "-o" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --out takes a path argument");
                    return 2;
                }
                out_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--trace-dir" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --trace-dir takes a path argument");
                    return 2;
                }
                trace_dir = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                eprintln!("error: unknown argument for `llm-loop`: {other}");
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let Some(command) = command else {
        eprintln!("error: `llm-loop` requires --command CMD");
        eprintln!("       e.g. --command python --arg llm_wrapper.py");
        return 2;
    };
    let prompt = match prompt_path.as_ref() {
        Some(p) => match fs::read_to_string(p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read prompt `{p}`: {e}");
                return 1;
            }
        },
        None => {
            // Read prompt from stdin so the command form composes
            // well: `cat task.md | twec llm-loop --command claude`.
            use std::io::Read;
            let mut s = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut s) {
                eprintln!("error: reading prompt from stdin failed: {e}");
                return 1;
            }
            s
        }
    };

    let mut provider = crate::llm_loop::CommandProvider::new(command, cmd_args);
    let options = crate::llm_loop::LoopOptions {
        max_rounds,
        trace_dir: trace_dir.map(std::path::PathBuf::from),
        source_path: out_path.clone(),
        log_prompts: true,
    };
    let outcome = match crate::llm_loop::run_loop(&mut provider, &prompt, &options) {
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
    eprintln!(
        "[twec llm-loop] {} after {} round(s){}",
        if outcome.passed { "PASSED" } else { "FAILED" },
        outcome.rounds.len(),
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

/// Phase 33 session 5: stub. The real handler is added when the
/// `mcp` module lands (next in this same commit).
fn handle_mcp(args: &[String]) -> i32 {
    if !args.is_empty() {
        eprintln!("error: `twec mcp` takes no arguments");
        return 2;
    }
    crate::mcp::serve_stdio()
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
    backend: Backend,
    frames: u32,
    path: Option<String>,
}

/// Shared --vm / --frames / positional-path parser. `allow_frames`
/// gates the `--frames N` flag (only `run` accepts it; `play` drives
/// frames from the macroquad clock).
fn parse_common_flags(args: &[String], allow_frames: bool) -> Result<CommonFlags, i32> {
    let mut backend = Backend::Tree;
    let mut frames: u32 = 0;
    let mut path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--vm" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --vm requires a backend (tree or bytecode)");
                    return Err(2);
                }
                backend = match Backend::parse(&args[i + 1]) {
                    Some(b) => b,
                    None => {
                        eprintln!(
                            "error: --vm value must be 'tree' or 'bytecode', got '{}'",
                            args[i + 1]
                        );
                        return Err(2);
                    }
                };
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
    Ok(CommonFlags {
        backend,
        frames,
        path,
    })
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
    match parsed.backend {
        Backend::Tree => run_file_tree(&path, parsed.frames),
        Backend::Bytecode => run_file_bytecode(&path, parsed.frames),
    }
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
    let result = if frames > 0 {
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

fn run_file_bytecode(path: &str, frames: u32) -> i32 {
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
    let chunk = match crate::compiler::compile_program(&program) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{path}: compile error: {e}");
            return 1;
        }
    };
    let mut vm = crate::vm::VM::new();
    if let Err(e) = vm.run(&chunk) {
        eprintln!("{path}: runtime error: {e}");
        return 1;
    }
    let dt = 1.0 / 60.0;
    for _ in 0..frames {
        if let Err(e) = vm.tick(dt) {
            eprintln!("{path}: runtime error: {e}");
            return 1;
        }
    }
    print!("{}", vm.out);
    0
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
    let path = dir.join(format!(
        "twec-crash-{secs}-{pid}.log",
        pid = std::process::id()
    ));
    let body = format_crash_body(info, secs);
    fs::write(&path, body).ok()?;
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
