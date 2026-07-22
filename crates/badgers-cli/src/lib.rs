mod baseline_fetch;
mod cov;
mod diff;
mod github_storage;
mod python;
mod render;
mod report;
mod report_github;
mod report_markdown;
mod setup_gcs;
mod snapshot_push;
mod storage_opts;
mod summary;
mod view;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

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
    /// Show the latest stored coverage diff for a pull request
    Diff(diff::DiffArgs),
    /// Run coverage locally and report uncovered changed or repo-wide lines
    Cov(cov::CovArgs),
    /// Download and open the latest stored HTML report for a pull request
    View(view::ViewArgs),
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

pub fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    match cli.command {
        Command::Collect(CollectCommand::Python(args)) => success(python::run(&args)),
        Command::Report(ReportCommand::Html(args)) => success(report::run(&args)),
        Command::Report(ReportCommand::Github(args)) => success(report_github::run(&args)),
        Command::Report(ReportCommand::Markdown(args)) => success(report_markdown::run(&args)),
        Command::Snapshot(SnapshotCommand::Push(args)) => success(snapshot_push::run(&args)),
        Command::Baseline(BaselineCommand::Fetch(args)) => success(baseline_fetch::run(&args)),
        Command::Setup(SetupCommand::Gcs(args)) => success(setup_gcs::run(&args)),
        Command::Diff(args) => success(diff::run(&args)),
        Command::Cov(args) => cov::run(&args),
        Command::View(args) => success(view::run(&args)),
    }
}

fn success(result: anyhow::Result<()>) -> anyhow::Result<ExitCode> {
    result.map(|()| ExitCode::SUCCESS)
}
