use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use sha2::{Digest, Sha256};

use crate::github_storage::{
    DEFAULT_STORAGE_BRANCH, DEFAULT_STORAGE_PREFIX, GithubReportLocation, checked_repo_path,
    clone_storage_branch, read_pointer, resolve_source_repo,
};

#[derive(Args, Debug)]
pub struct ViewArgs {
    /// Pull request number whose latest HTML report should be opened
    pub pr: u64,

    /// Source repository, inferred from GITHUB_REPOSITORY or the local origin remote
    #[arg(long, value_name = "OWNER/REPO")]
    pub repo: Option<String>,

    /// Repository containing the durable report branch (defaults to --repo)
    #[arg(long, value_name = "OWNER/REPO")]
    pub storage_repo: Option<String>,

    /// Dedicated branch containing durable reports
    #[arg(long, default_value = DEFAULT_STORAGE_BRANCH)]
    pub storage_branch: String,

    /// Path prefix inside the durable report branch
    #[arg(long, default_value = DEFAULT_STORAGE_PREFIX)]
    pub storage_prefix: String,

    /// Directory used to cache downloaded HTML bundles
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Download the report and print its path without opening a browser
    #[arg(long)]
    pub no_open: bool,
}

pub fn run(args: &ViewArgs) -> Result<()> {
    ensure!(args.pr > 0, "pull request number must be greater than zero");
    let source = resolve_source_repo(args.repo.as_deref())?;
    let storage_repo = args.storage_repo.as_deref().unwrap_or(&source.slug);
    let location = GithubReportLocation::new(
        &source.slug,
        storage_repo,
        &args.storage_branch,
        &args.storage_prefix,
    )?;

    let checkout = tempfile::tempdir().context("creating temporary GitHub storage checkout")?;
    clone_storage_branch(&location, source.origin_url.as_deref(), checkout.path())?;
    let pointer = read_pointer(&location, args.pr, checkout.path())?;
    let html_prefix = pointer
        .html_prefix
        .as_deref()
        .context("the latest PR report does not contain an HTML bundle")?;
    let expected_prefix = location.html_prefix(&pointer.commit_sha)?;
    ensure!(
        html_prefix == expected_prefix,
        "PR pointer HTML path does not match commit {}",
        pointer.commit_sha
    );

    let source_html = checked_repo_path(checkout.path(), html_prefix)?;
    ensure!(
        source_html.is_dir(),
        "stored HTML report is not a directory"
    );
    let source_index = checked_repo_path(checkout.path(), &format!("{html_prefix}/index.html"))?;
    ensure!(
        source_index.is_file(),
        "stored HTML report has no index.html"
    );

    let cache_root = args
        .cache_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("badgers-view"));
    let destination = cached_report_dir(&cache_root, &location, args.pr, &pointer.commit_sha);
    materialize_report(&source_html, &destination)?;
    let index = destination.join("index.html");

    println!(
        "stored report: {}",
        location.report_spec(&pointer.commit_sha)?
    );
    println!("local report: {}", index.display());
    if !args.no_open {
        open_browser(&index)?;
    }
    Ok(())
}

fn cached_report_dir(root: &Path, location: &GithubReportLocation, pr: u64, sha: &str) -> PathBuf {
    let identity = format!(
        "{}\n{}\n{}\n{}",
        location.storage_repo,
        location.storage_branch,
        location.storage_prefix,
        location.source_repo
    );
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    root.join(&digest[..16])
        .join("prs")
        .join(pr.to_string())
        .join(sha)
        .join("html")
}

fn materialize_report(source: &Path, destination: &Path) -> Result<()> {
    if destination.join("index.html").is_file() {
        return Ok(());
    }
    let parent = destination
        .parent()
        .context("cached report destination has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating report cache {}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".html-")
        .tempdir_in(parent)
        .with_context(|| format!("creating staging directory in {}", parent.display()))?;
    copy_report_tree(source, staging.path())?;
    let staging_path = staging.keep();
    match std::fs::rename(&staging_path, destination) {
        Ok(()) => Ok(()),
        Err(_error) if destination.join("index.html").is_file() => {
            std::fs::remove_dir_all(&staging_path).with_context(|| {
                format!(
                    "removing redundant staging directory {}",
                    staging_path.display()
                )
            })?;
            Ok(())
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "moving downloaded report from {} to {}",
                staging_path.display(),
                destination.display()
            )
        }),
    }
}

fn copy_report_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in std::fs::read_dir(source)
        .with_context(|| format!("reading stored HTML directory {}", source.display()))?
    {
        let entry = entry.with_context(|| format!("iterating {}", source.display()))?;
        let name = entry.file_name();
        validate_filename(&name)?;
        let source_path = entry.path();
        let destination_path = destination.join(&name);
        let metadata = std::fs::symlink_metadata(&source_path)
            .with_context(|| format!("reading metadata for {}", source_path.display()))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "stored HTML report contains a symlink: {}",
            source_path.display()
        );
        if metadata.is_dir() {
            std::fs::create_dir(&destination_path)
                .with_context(|| format!("creating {}", destination_path.display()))?;
            copy_report_tree(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copying stored report file {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        } else {
            bail!(
                "stored HTML report contains a special file: {}",
                source_path.display()
            );
        }
    }
    Ok(())
}

fn validate_filename(name: &OsStr) -> Result<()> {
    let name = name
        .to_str()
        .context("stored HTML report contains a non-UTF-8 filename")?;
    ensure!(
        !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\']),
        "stored HTML report contains an unsafe filename"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn browser_command(path: &Path) -> Command {
    let mut command = Command::new("open");
    command.arg(path);
    command
}

#[cfg(target_os = "linux")]
fn browser_command(path: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(path);
    command
}

#[cfg(target_os = "windows")]
fn browser_command(path: &Path) -> Command {
    let mut command = Command::new("cmd");
    command.args(["/C", "start", ""]).arg(path);
    command
}

fn open_browser(path: &Path) -> Result<()> {
    let status = browser_command(path)
        .status()
        .with_context(|| format!("opening {} in the default browser", path.display()))?;
    ensure!(status.success(), "the default browser could not be opened");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn location() -> GithubReportLocation {
        GithubReportLocation::new(
            "owner/project",
            "reports/archive",
            DEFAULT_STORAGE_BRANCH,
            DEFAULT_STORAGE_PREFIX,
        )
        .unwrap()
    }

    #[test]
    fn copies_report_into_commit_scoped_cache() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("index.html"), "index").unwrap();
        std::fs::write(source.join("nested/file.html"), "file").unwrap();
        let destination = cached_report_dir(temp.path(), &location(), 547, SHA);
        materialize_report(&source, &destination).unwrap();
        assert_eq!(
            std::fs::read_to_string(destination.join("index.html")).unwrap(),
            "index"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("nested/file.html")).unwrap(),
            "file"
        );
    }
}
