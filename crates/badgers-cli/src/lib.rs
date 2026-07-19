mod baseline_fetch;
mod python;
mod report;
mod report_github;
mod snapshot_push;
mod storage_opts;
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
    #[command(subcommand)]
    Snapshot(SnapshotCommand),
    #[command(subcommand)]
    Baseline(BaselineCommand),
}

#[derive(Subcommand)]
pub enum CollectCommand {
    Python(python::CollectPythonArgs),
}

#[derive(Subcommand)]
pub enum ReportCommand {
    Html(report::HtmlArgs),
    Github(report_github::GithubArgs),
}

#[derive(Subcommand)]
pub enum SnapshotCommand {
    Push(snapshot_push::SnapshotPushArgs),
}

#[derive(Subcommand)]
pub enum BaselineCommand {
    Fetch(baseline_fetch::BaselineFetchArgs),
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Collect(CollectCommand::Python(args)) => python::run(&args),
        Command::Report(ReportCommand::Html(args)) => report::run(&args),
        Command::Report(ReportCommand::Github(args)) => report_github::run(&args),
        Command::Snapshot(SnapshotCommand::Push(args)) => snapshot_push::run(&args),
        Command::Baseline(BaselineCommand::Fetch(args)) => baseline_fetch::run(&args),
    }
}
