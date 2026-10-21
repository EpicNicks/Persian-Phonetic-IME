//! Developer tasks: `build-dict`, `check-dict`, `eval-dict` (docs/autocomplete.md sections 7, 9).
mod dict;
mod eval;

use std::process::ExitCode;

const USAGE: &str = "usage: xtask <command> ...
  build-dict --input <tsv> [--input <tsv>...] --out <path> [--min-zipf N] [--max-entries N]
             [--max-bytes N] [--info key=value ...]
  check-dict <path>
  eval-dict ...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build-dict") => dict::build(&args[1..]),
        Some("check-dict") => dict::check(&args[1..]),
        Some("eval-dict") => eval::run(&args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(2)
        }
    }
}
