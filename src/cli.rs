use std::env;
use std::fs;
use std::process;

const USAGE: &str =
    "usage: twec [run [--vm tree|bytecode] [--frames N] <file> | \
     play [--vm tree|bytecode] <file> | \
     fmt [--in-place|--check] <file> | \
     parse <file> | version]";

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
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_version();
        return;
    }
    match args[1].as_str() {
        "run" => process::exit(handle_run(&args[2..])),
        "play" => process::exit(handle_play(&args[2..])),
        "fmt" => process::exit(handle_fmt(&args[2..])),
        "parse" => process::exit(handle_parse(&args[2..])),
        "version" | "--version" | "-V" => print_version(),
        cmd => {
            eprintln!("error: unknown command '{cmd}'");
            eprintln!("{USAGE}");
            process::exit(2);
        }
    }
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
    match parsed.backend {
        Backend::Tree => crate::play::launch(path),
        Backend::Bytecode => crate::play::launch_bytecode(path),
    }
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
    let formatted = crate::printer::print_program(&program);
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
    Ok(CommonFlags { backend, frames, path })
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
