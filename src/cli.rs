use std::env;
use std::fs;
use std::process;

const USAGE: &str = "usage: twec [run <file> | version]";

pub fn run() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_version();
        return;
    }
    match args[1].as_str() {
        "run" => {
            if args.len() < 3 {
                eprintln!("error: `twec run` requires a file path");
                eprintln!("{USAGE}");
                process::exit(2);
            }
            process::exit(run_file(&args[2]));
        }
        "version" | "--version" | "-V" => print_version(),
        cmd => {
            eprintln!("error: unknown command '{cmd}'");
            eprintln!("{USAGE}");
            process::exit(2);
        }
    }
}

fn print_version() {
    println!("twec {}", env!("CARGO_PKG_VERSION"));
}

fn run_file(path: &str) -> i32 {
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
    match crate::eval::run(&program) {
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
