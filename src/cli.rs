use std::env;
use std::fs;
use std::process;

const USAGE: &str = "usage: twec [run [--frames N] <file> | parse <file> | version]";

pub fn run() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_version();
        return;
    }
    match args[1].as_str() {
        "run" => process::exit(handle_run(&args[2..])),
        "parse" => process::exit(handle_parse(&args[2..])),
        "version" | "--version" | "-V" => print_version(),
        cmd => {
            eprintln!("error: unknown command '{cmd}'");
            eprintln!("{USAGE}");
            process::exit(2);
        }
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

fn handle_run(args: &[String]) -> i32 {
    let mut frames: u32 = 0;
    let mut path: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frames" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --frames requires a number");
                    return 2;
                }
                frames = match args[i + 1].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("error: --frames value must be a non-negative integer");
                        return 2;
                    }
                };
                i += 2;
            }
            other if other.starts_with("--") => {
                eprintln!("error: unknown flag '{other}'");
                eprintln!("{USAGE}");
                return 2;
            }
            other => {
                if path.is_some() {
                    eprintln!("error: `twec run` takes a single file path");
                    return 2;
                }
                path = Some(other);
                i += 1;
            }
        }
    }
    let Some(path) = path else {
        eprintln!("error: `twec run` requires a file path");
        eprintln!("{USAGE}");
        return 2;
    };
    run_file(path, frames)
}

fn run_file(path: &str, frames: u32) -> i32 {
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
