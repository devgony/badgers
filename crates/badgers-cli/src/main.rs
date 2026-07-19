use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = badgers_cli::Cli::parse();
    match badgers_cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}
