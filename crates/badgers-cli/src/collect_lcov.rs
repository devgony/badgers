use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use badge_rs_core::{CoverageSnapshot, ToolVersions};
use badge_rs_lcov::{ParseOptions, enrich_mcdc_from_llvm_json, parse_lcov};
use clap::Args;

#[derive(Args, Debug)]
pub struct CollectLcovArgs {
    /// LCOV file produced by your coverage tool
    #[arg(long, value_name = "PATH", default_value = "coverage/lcov.info")]
    pub lcov_file: PathBuf,

    /// Optional llvm-cov JSON export (-format=text) used to add MC/DC data,
    /// which llvm-cov omits from its LCOV output
    #[arg(long, value_name = "PATH")]
    pub llvm_cov_json: Option<PathBuf>,

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

pub fn run(args: &CollectLcovArgs) -> Result<()> {
    let repo_root = fs::canonicalize(&args.repo_root)
        .with_context(|| format!("repo root '{}' not found", args.repo_root.display()))?;

    let lcov_text = fs::read_to_string(&args.lcov_file).with_context(|| {
        format!(
            "failed to read LCOV file '{}' (did your coverage command write it?)",
            args.lcov_file.display()
        )
    })?;

    let mut outcome = parse_lcov(
        &lcov_text,
        &ParseOptions {
            repo_root: &repo_root,
        },
    )?;
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }

    if let Some(llvm_cov_json) = &args.llvm_cov_json {
        let json_text = fs::read_to_string(llvm_cov_json).with_context(|| {
            format!(
                "failed to read llvm-cov JSON file '{}' (did your coverage command write it?)",
                llvm_cov_json.display()
            )
        })?;
        let warnings = enrich_mcdc_from_llvm_json(
            &json_text,
            &ParseOptions {
                repo_root: &repo_root,
            },
            &mut outcome.files,
        )?;
        for warning in &warnings {
            eprintln!("warning: {warning}");
        }
    }

    let snapshot = CoverageSnapshot::new(
        std::env::var("GITHUB_REPOSITORY").unwrap_or_default(),
        checkout_sha(&repo_root),
        None,
        None,
        jiff::Timestamp::now().to_string(),
        ToolVersions {
            badgers: env!("CARGO_PKG_VERSION").to_string(),
            cargo_llvm_cov: None,
            coverage_py: None,
        },
        outcome.files,
    );

    let json = serde_json::to_string_pretty(&snapshot)?;
    fs::write(&args.output, json + "\n")
        .with_context(|| format!("failed to write snapshot to '{}'", args.output.display()))?;

    print!("{}", crate::summary::render(&snapshot));
    Ok(())
}

fn checkout_sha(repo_root: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output();
    if let Ok(output) = output
        && output.status.success()
    {
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !sha.is_empty() {
            return sha;
        }
    }
    std::env::var("GITHUB_SHA").unwrap_or_default()
}
