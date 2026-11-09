//! `ultra-instinct` command line.

#![forbid(unsafe_code)]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "mcp") {
        if args.len() != 1 {
            eprintln!("ultra-instinct: mcp takes no arguments");
            std::process::exit(2);
        }
        if let Err(err) = ultra_instinct_mcp::serve_stdio() {
            eprintln!("ultra-instinct: {err}");
            std::process::exit(1);
        }
        return;
    }
    match ultra_instinct::execute(&args) {
        Ok(stdout) => print!("{stdout}"),
        // A batch with a failed command: every command's output, then exit 1.
        Err(ultra_instinct::CliError::BatchFailed(body)) => {
            print!("{body}");
            std::process::exit(1);
        }
        // A shell-verb refusal is already one short line.
        Err(err @ ultra_instinct::CliError::Refused(_)) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("ultra-instinct: {err}");
            std::process::exit(1);
        }
    }
}
