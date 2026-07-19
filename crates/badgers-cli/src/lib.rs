mod baseline_fetch;
mod python;
mod report;
mod report_github;
mod report_markdown;
mod setup_gcs;
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
    #[command(subcommand)]
    Setup(SetupCommand),
}

#[derive(Subcommand)]
pub enum SetupCommand {
    Gcs(setup_gcs::SetupGcsArgs),
}

#[derive(Subcommand)]
pub enum CollectCommand {
    Python(python::CollectPythonArgs),
}

#[derive(Subcommand)]
pub enum ReportCommand {
    Html(report::HtmlArgs),
    Github(report_github::GithubArgs),
    Markdown(report_markdown::MarkdownArgs),
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
        Command::Report(ReportCommand::Markdown(args)) => report_markdown::run(&args),
        Command::Snapshot(SnapshotCommand::Push(args)) => snapshot_push::run(&args),
        Command::Baseline(BaselineCommand::Fetch(args)) => baseline_fetch::run(&args),
        Command::Setup(SetupCommand::Gcs(args)) => setup_gcs::run(&args),
    }
}
