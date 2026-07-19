mod python;
mod report;
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
    #[command(subcommand)]
    Report(ReportCommand),
}

#[derive(Subcommand)]
pub enum CollectCommand {
    Python(python::CollectPythonArgs),
}

#[derive(Subcommand)]
pub enum ReportCommand {
    Html(report::HtmlArgs),
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Collect(CollectCommand::Python(args)) => python::run(&args),
        Command::Report(ReportCommand::Html(args)) => report::run(&args),
    }
}
