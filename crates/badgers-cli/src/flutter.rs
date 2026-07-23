use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use badge_rs_core::{CoverageSnapshot, ToolVersions};
use badge_rs_lcov::{ParseOptions, parse_lcov};
use clap::Args;

#[derive(Args, Debug)]
pub struct CollectFlutterArgs {
    /// Parse this LCOV file instead of the default `coverage/lcov.info`
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

pub fn run(args: &CollectFlutterArgs) -> Result<()> {
    let repo_root = fs::canonicalize(&args.repo_root)
        .with_context(|| format!("repo root '{}' not found", args.repo_root.display()))?;

    let (lcov_text, flutter_version) = match &args.lcov_file {
        Some(path) => {
            let text = fs::read_to_string(path)
                .with_context(|| format!("failed to read LCOV file '{}'", path.display()))?;
            (text, None)
        }
        None => {
            let default_path = repo_root.join("coverage").join("lcov.info");
            let text = fs::read_to_string(&default_path).with_context(|| {
                format!(
                    "failed to read '{}' (run `flutter test --coverage` first)",
                    default_path.display()
                )
            })?;
            (text, detect_flutter_version(&repo_root))
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
        crate::python::checkout_sha(&repo_root),
        None,
        None,
        jiff::Timestamp::now().to_string(),
        ToolVersions {
            badgers: env!("CARGO_PKG_VERSION").to_string(),
            cargo_llvm_cov: None,
            coverage_py: None,
            flutter: flutter_version,
        },
        outcome.files,
    );

    let json = serde_json::to_string_pretty(&snapshot)?;
    fs::write(&args.output, json + "\n")
        .with_context(|| format!("failed to write snapshot to '{}'", args.output.display()))?;

    print!("{}", crate::summary::render(&snapshot));
    Ok(())
}

fn detect_flutter_version(repo_root: &Path) -> Option<String> {
    let output = Command::new("flutter")
        .args(["--version", "--machine"])
        .current_dir(repo_root)
        .output();
    let version = output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| parse_flutter_version(&String::from_utf8_lossy(&output.stdout)));
    if version.is_none() {
        eprintln!("warning: could not detect the Flutter SDK version");
    }
    version
}

fn parse_flutter_version(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let version = value.get("frameworkVersion")?.as_str()?;
    (!version.is_empty()).then(|| version.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_flutter_version;

    #[test]
    fn parses_framework_version_from_machine_output() {
        let json = r#"{"frameworkVersion":"3.32.0","channel":"stable","dartSdkVersion":"3.8.0"}"#;
        assert_eq!(parse_flutter_version(json), Some("3.32.0".to_string()));
    }

    #[test]
    fn rejects_missing_or_empty_version() {
        assert_eq!(parse_flutter_version(r#"{"channel":"stable"}"#), None);
        assert_eq!(parse_flutter_version(r#"{"frameworkVersion":""}"#), None);
        assert_eq!(parse_flutter_version("not json"), None);
    }
}
