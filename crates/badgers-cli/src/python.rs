use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use badge_rs_core::{CoverageSnapshot, ToolVersions};
use badge_rs_lcov::{ParseOptions, parse_lcov};
use clap::Args;

const MIN_COVERAGE_PY: (u32, u32) = (6, 3);

#[derive(Args, Debug)]
pub struct CollectPythonArgs {
    /// Parse an existing LCOV file instead of invoking `coverage lcov`
    #[arg(long, value_name = "PATH")]
    pub lcov_file: Option<PathBuf>,

    /// Repository root used to normalize file paths
    #[arg(long, value_name = "PATH", default_value = ".")]
    pub repo_root: PathBuf,

    /// Where to write the coverage snapshot JSON
    #[arg(
        short,
        long,
        value_name = "PATH",
        default_value = "coverage-snapshot.json"
    )]
    pub output: PathBuf,
}

pub fn run(args: &CollectPythonArgs) -> Result<()> {
    let repo_root = fs::canonicalize(&args.repo_root)
        .with_context(|| format!("repo root '{}' not found", args.repo_root.display()))?;

    let (lcov_text, coverage_py_version) = match &args.lcov_file {
        Some(path) => {
            let text = fs::read_to_string(path)
                .with_context(|| format!("failed to read LCOV file '{}'", path.display()))?;
            (text, None)
        }
        None => {
            let version = detect_coverage_version(&repo_root)?;
            (run_coverage_lcov(&repo_root)?, Some(version))
        }
    };

    let outcome = parse_lcov(
        &lcov_text,
        &ParseOptions {
            repo_root: &repo_root,
        },
    )?;
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }

    let snapshot = CoverageSnapshot::new(
        std::env::var("GITHUB_REPOSITORY").unwrap_or_default(),
        std::env::var("GITHUB_SHA").unwrap_or_default(),
        None,
        None,
        jiff::Timestamp::now().to_string(),
        ToolVersions {
            badgers: env!("CARGO_PKG_VERSION").to_string(),
            cargo_llvm_cov: None,
            coverage_py: coverage_py_version,
        },
        outcome.files,
    );

    let json = serde_json::to_string_pretty(&snapshot)?;
    fs::write(&args.output, json + "\n")
        .with_context(|| format!("failed to write snapshot to '{}'", args.output.display()))?;

    print!("{}", crate::summary::render(&snapshot));
    Ok(())
}

fn detect_coverage_version(repo_root: &Path) -> Result<String> {
    let output = Command::new("python3")
        .args(["-m", "coverage", "--version"])
        .current_dir(repo_root)
        .output()
        .context("failed to invoke python3 - is Python installed?")?;
    if !output.status.success() {
        bail!(
            "`python3 -m coverage --version` failed - is coverage.py installed? ({})",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (major, minor, version) = parse_coverage_version(&stdout).with_context(|| {
        format!(
            "could not parse coverage.py version from: {}",
            stdout.trim()
        )
    })?;
    if (major, minor) < MIN_COVERAGE_PY {
        bail!(
            "coverage.py >= {}.{} is required for `coverage lcov` (found {version})",
            MIN_COVERAGE_PY.0,
            MIN_COVERAGE_PY.1
        );
    }
    Ok(version)
}

fn run_coverage_lcov(repo_root: &Path) -> Result<String> {
    let tmp = std::env::temp_dir().join(format!("badgers-python-{}.lcov", std::process::id()));
    let output = Command::new("python3")
        .args(["-m", "coverage", "lcov", "-o"])
        .arg(&tmp)
        .current_dir(repo_root)
        .output()
        .context("failed to invoke python3")?;
    if !output.status.success() {
        bail!(
            "`python3 -m coverage lcov` failed (did you run `coverage run` first?): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = fs::read_to_string(&tmp)
        .with_context(|| format!("failed to read generated LCOV at '{}'", tmp.display()))?;
    let _ = fs::remove_file(&tmp);
    Ok(text)
}

/// Parses output like "Coverage.py, version 7.6.1 with C extension".
fn parse_coverage_version(s: &str) -> Option<(u32, u32, String)> {
    for token in s.split_whitespace() {
        let token = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        if token.chars().next().is_some_and(|c| c.is_ascii_digit()) && token.contains('.') {
            let mut parts = token.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            return Some((major, minor, token.to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_coverage_version;

    #[test]
    fn parses_standard_version_output() {
        let (major, minor, version) =
            parse_coverage_version("Coverage.py, version 7.6.1 with C extension").unwrap();
        assert_eq!((major, minor), (7, 6));
        assert_eq!(version, "7.6.1");
    }

    #[test]
    fn parses_two_part_version() {
        let (major, minor, version) =
            parse_coverage_version("Coverage.py, version 6.3 without C extension").unwrap();
        assert_eq!((major, minor), (6, 3));
        assert_eq!(version, "6.3");
    }

    #[test]
    fn rejects_output_without_version() {
        assert!(parse_coverage_version("no version here").is_none());
    }
}
