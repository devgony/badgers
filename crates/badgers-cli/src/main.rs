use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = badge_rs::Cli::parse();
    match badge_rs::run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}
