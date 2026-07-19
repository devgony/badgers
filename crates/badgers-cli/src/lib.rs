mod python;
mod summary;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "badgers",
    version,
    about = "Coverage checker for Rust and Python projects"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(subcommand)]
    Collect(CollectCommand),
}

#[derive(Subcommand)]
pub enum CollectCommand {
    Python(python::CollectPythonArgs),
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Collect(CollectCommand::Python(args)) => python::run(&args),
    }
}
