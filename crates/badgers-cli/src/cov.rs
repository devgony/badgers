use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail, ensure};
use badge_rs_core::compare::{ChangedLines, Comparison, compare};
use badge_rs_core::diff::parse_unified_diff;
use badge_rs_core::{CoverageSnapshot, ToolVersions};
use badge_rs_lcov::{ParseOptions, parse_lcov};
use clap::Args;

use crate::render::{
    RenderOptions, render_comparison, render_comparison_with_options, uncovered_count,
};
use crate::report::{git_diff_output, read_snapshot};

#[derive(Args, Debug)]
pub struct CovArgs {
    /// Git ref to diff against (default: resolved automatically)
    #[arg(long, value_name = "REF")]
    pub base_ref: Option<String>,

    /// Report all uncovered executable lines without diffing against a base ref
    #[arg(long, conflicts_with = "base_ref")]
    pub all: bool,

    /// Only include repo-relative paths under this prefix; filters all displayed totals and exit status
    #[arg(long, value_name = "PATH")]
    pub path: Vec<String>,

    /// Use an existing LCOV file instead of running coverage
    #[arg(long, value_name = "PATH")]
    pub lcov_file: Option<PathBuf>,

    /// Optional base coverage snapshot JSON for the filtered total-coverage delta
    #[arg(long, value_name = "PATH")]
    pub baseline: Option<PathBuf>,

    /// Repository root
    #[arg(long, value_name = "PATH", default_value = ".")]
    pub repo_root: PathBuf,

    /// Always exit 0, even when displayed uncovered lines remain
    #[arg(long)]
    pub no_fail: bool,
}

pub fn run(args: &CovArgs) -> Result<ExitCode> {
    let repo_root = fs::canonicalize(&args.repo_root)
        .with_context(|| format!("repo root '{}' not found", args.repo_root.display()))?;
    let changed = if args.all {
        None
    } else {
        let base_ref = resolve_base_ref(&repo_root, args.base_ref.as_deref())?;
        let merge_base = git_output(
            &repo_root,
            &["merge-base", &base_ref, "HEAD"],
            &format!("finding merge base for {base_ref} and HEAD"),
        )?;
        ensure!(
            !merge_base.is_empty(),
            "git merge-base returned an empty commit SHA"
        );
        Some(parse_unified_diff(&git_diff_output(
            &repo_root,
            &merge_base,
        )?))
    };
    let head = head_snapshot(args, &repo_root)?;
    let changed = changed.unwrap_or_else(|| all_executable_lines(&head));
    let base = args.baseline.as_deref().map(read_snapshot).transpose()?;
    let mut comparison = compare(base.as_ref(), &head, &changed);
    filter_comparison(&mut comparison, &args.path);

    let branch = git_output(
        &repo_root,
        &["rev-parse", "--abbrev-ref", "HEAD"],
        "resolving the current branch",
    )?;
    let dirty = !git_output(
        &repo_root,
        &["status", "--porcelain"],
        "checking the working tree status",
    )?
    .is_empty();
    let short_sha = &head.commit_sha[..head.commit_sha.len().min(7)];
    let dirty_marker = if dirty { " (dirty)" } else { "" };
    let context = format!("Local: {branch} @ {short_sha}{dirty_marker}");
    let rendered = if args.all {
        render_comparison_with_options(&context, &comparison, RenderOptions::REPO_WIDE)
    } else {
        render_comparison(&context, &comparison)
    };
    print!("{rendered}");

    Ok(comparison_exit_code(&comparison, args.no_fail))
}

fn all_executable_lines(snapshot: &CoverageSnapshot) -> ChangedLines {
    ChangedLines(
        snapshot
            .files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    file.line_hits
                        .iter()
                        .map(|line_hit| line_hit.line)
                        .collect(),
                )
            })
            .collect(),
    )
}

fn filter_comparison(comparison: &mut Comparison, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let prefixes: Vec<String> = paths.iter().map(|path| normalize_prefix(path)).collect();
    comparison.files.retain(|file| {
        prefixes
            .iter()
            .any(|prefix| path_matches(&file.path, prefix))
    });
}

fn normalize_prefix(prefix: &str) -> String {
    prefix
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

fn path_matches(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn resolve_base_ref(repo_root: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(base_ref) = explicit {
        return Ok(base_ref.to_string());
    }

    let gh_base = Command::new("gh")
        .args([
            "pr",
            "view",
            "--json",
            "baseRefName",
            "--jq",
            ".baseRefName",
        ])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|base| !base.is_empty());
    if let Some(base) = gh_base {
        return Ok(format!("origin/{base}"));
    }

    let origin_head = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|base| !base.is_empty());
    origin_head.context(
        "could not determine the base ref from the current pull request or origin/HEAD; pass --base-ref <REF>",
    )
}

fn head_snapshot(args: &CovArgs, repo_root: &Path) -> Result<CoverageSnapshot> {
    let lcov = match &args.lcov_file {
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("failed to read LCOV file '{}'", path.display()))?,
        None => run_coverage(repo_root)?,
    };
    let outcome = parse_lcov(&lcov, &ParseOptions { repo_root })?;
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }

    let commit_sha = git_output(
        repo_root,
        &["rev-parse", "HEAD"],
        "resolving the current commit",
    )?;
    ensure!(
        !commit_sha.is_empty(),
        "git rev-parse HEAD returned an empty commit SHA"
    );
    Ok(CoverageSnapshot::new(
        std::env::var("GITHUB_REPOSITORY").unwrap_or_default(),
        commit_sha,
        None,
        None,
        jiff::Timestamp::now().to_string(),
        ToolVersions {
            badgers: env!("CARGO_PKG_VERSION").to_string(),
            cargo_llvm_cov: None,
            coverage_py: None,
            flutter: None,
        },
        outcome.files,
    ))
}

fn run_coverage(repo_root: &Path) -> Result<String> {
    let output_file =
        tempfile::NamedTempFile::new().context("creating a temporary file for generated LCOV")?;
    let path = output_file.path();

    if repo_root.join("Cargo.toml").is_file() {
        let output = Command::new("cargo")
            .args(["llvm-cov", "--workspace", "--lcov", "--output-path"])
            .arg(path)
            .current_dir(repo_root)
            .output()
            .context(
                "failed to invoke cargo llvm-cov; install it with `cargo install cargo-llvm-cov`",
            )?;
        if !output.status.success() {
            bail!(
                "`cargo llvm-cov --workspace --lcov` failed; install it with `cargo install cargo-llvm-cov`: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    } else if repo_root.join("pubspec.yaml").is_file() {
        let output = Command::new("flutter")
            .args(["test", "--coverage"])
            .current_dir(repo_root)
            .output()
            .context("failed to invoke flutter - is the Flutter SDK installed?")?;
        if !output.status.success() {
            bail!(
                "`flutter test --coverage` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let lcov_path = repo_root.join("coverage").join("lcov.info");
        return fs::read_to_string(&lcov_path).with_context(|| {
            format!("failed to read generated LCOV at '{}'", lcov_path.display())
        });
    } else {
        let output = Command::new("python3")
            .args(["-m", "coverage", "lcov", "-o"])
            .arg(path)
            .current_dir(repo_root)
            .output()
            .context("failed to invoke python3 for coverage; did you run `coverage run` first?")?;
        if !output.status.success() {
            bail!(
                "`python3 -m coverage lcov` failed (did you run `coverage run` first?): {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }

    fs::read_to_string(path)
        .with_context(|| format!("failed to read generated LCOV at '{}'", path.display()))
}

fn git_output(repo_root: &Path, args: &[&str], action: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .with_context(|| format!("failed to invoke git while {action}"))?;
    if !output.status.success() {
        bail!(
            "git failed while {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn comparison_exit_code(comparison: &Comparison, no_fail: bool) -> ExitCode {
    if !no_fail && uncovered_count(comparison) > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use badge_rs_core::compare::ChangedLines;
    use badge_rs_core::{FileCoverage, Language, LineHit};

    use super::*;

    fn comparison_with_uncovered_line() -> Comparison {
        let head = CoverageSnapshot::new(
            "owner/repo".into(),
            "0123456789abcdef0123456789abcdef01234567".into(),
            None,
            None,
            "2026-07-21T00:00:00Z".into(),
            ToolVersions {
                badgers: "0.1.1".into(),
                cargo_llvm_cov: None,
                coverage_py: None,
                flutter: None,
            },
            vec![FileCoverage::new(
                "src/lib.rs".into(),
                Language::Rust,
                vec![LineHit { line: 10, hits: 1 }, LineHit { line: 11, hits: 0 }],
            )],
        );
        let changed = ChangedLines(BTreeMap::from([(
            "src/lib.rs".into(),
            BTreeSet::from([10, 11]),
        )]));
        compare(None, &head, &changed)
    }

    fn snapshot(files: Vec<FileCoverage>) -> CoverageSnapshot {
        CoverageSnapshot::new(
            "owner/repo".into(),
            "0123456789abcdef0123456789abcdef01234567".into(),
            None,
            None,
            "2026-07-21T00:00:00Z".into(),
            ToolVersions {
                badgers: "0.1.1".into(),
                cargo_llvm_cov: None,
                coverage_py: None,
                flutter: None,
            },
            files,
        )
    }

    fn file(path: &str, hits: &[(u32, u64)]) -> FileCoverage {
        FileCoverage::new(
            path.into(),
            Language::from_path(path),
            hits.iter()
                .map(|&(line, hits)| LineHit { line, hits })
                .collect(),
        )
    }

    #[test]
    fn renders_local_dirty_header_and_comparison() {
        let comparison = comparison_with_uncovered_line();
        assert_eq!(
            render_comparison("Local: feat/my-branch @ 0123456 (dirty)", &comparison),
            "Coverage diff: 1 uncovered changed executable line\n\
Local: feat/my-branch @ 0123456 (dirty)\n\
Total coverage: 50.00% (no baseline)\n\
Changed-line coverage: 50.00% (1/2)\n\
src/lib.rs:11 [changed-uncovered]\n"
        );
    }

    #[test]
    fn uncovered_lines_control_exit_code_unless_no_fail_is_set() {
        let comparison = comparison_with_uncovered_line();
        assert_eq!(comparison_exit_code(&comparison, false), ExitCode::from(1));
        assert_eq!(comparison_exit_code(&comparison, true), ExitCode::SUCCESS);

        let clean = Comparison {
            base_available: false,
            files: Vec::new(),
        };
        assert_eq!(comparison_exit_code(&clean, false), ExitCode::SUCCESS);
    }

    #[test]
    fn renders_repo_wide_uncovered_lines() {
        let comparison = comparison_with_uncovered_line();
        assert_eq!(
            render_comparison_with_options(
                "Local: main @ 0123456 (dirty)",
                &comparison,
                RenderOptions::REPO_WIDE,
            ),
            "Coverage: 1 uncovered executable line\n\
Local: main @ 0123456 (dirty)\n\
Total coverage: 50.00% (no baseline)\n\
src/lib.rs:11 [uncovered]\n"
        );
    }

    #[test]
    fn renders_clean_repo_wide_coverage() {
        let head = snapshot(vec![file("src/lib.rs", &[(10, 1), (11, 2)])]);
        let comparison = compare(None, &head, &all_executable_lines(&head));
        assert_eq!(
            render_comparison_with_options(
                "Local: main @ 0123456",
                &comparison,
                RenderOptions::REPO_WIDE,
            ),
            "Coverage: no uncovered executable lines\n\
Local: main @ 0123456\n\
Total coverage: 100.00% (no baseline)\n"
        );
    }

    #[test]
    fn path_filter_controls_rendered_files_totals_and_exit_code() {
        let base = snapshot(vec![
            file("crates/badgers-cli/src/cov.rs", &[(10, 0), (11, 0)]),
            file("crates/badgers-core/src/lib.rs", &[(20, 1), (21, 1)]),
            file("covered/src/lib.rs", &[(30, 1)]),
        ]);
        let head = snapshot(vec![
            file("crates/badgers-cli/src/cov.rs", &[(10, 1), (11, 0)]),
            file("crates/badgers-core/src/lib.rs", &[(20, 0), (21, 0)]),
            file("covered/src/lib.rs", &[(30, 1)]),
        ]);
        let changed = all_executable_lines(&head);
        let mut comparison = compare(Some(&base), &head, &changed);
        filter_comparison(&mut comparison, &["./crates/badgers-cli/".into()]);

        assert_eq!(
            render_comparison("Local: main @ 0123456", &comparison),
            "Coverage diff: 1 uncovered changed executable line\n\
Local: main @ 0123456\n\
Total coverage: 50.00% (+50.00pp)\n\
Changed-line coverage: 50.00% (1/2)\n\
crates/badgers-cli/src/cov.rs:11 [changed-uncovered]\n"
        );
        assert_eq!(uncovered_count(&comparison), 1);
        assert_eq!(comparison_exit_code(&comparison, false), ExitCode::from(1));

        let mut covered_only = compare(Some(&base), &head, &changed);
        filter_comparison(&mut covered_only, &["covered".into()]);
        assert_eq!(
            comparison_exit_code(&covered_only, false),
            ExitCode::SUCCESS
        );
    }

    #[test]
    fn path_prefixes_match_component_boundaries() {
        assert!(path_matches(
            "src/foo/lib.rs",
            &normalize_prefix("src/foo/")
        ));
        assert!(path_matches("src/foo", &normalize_prefix("./src/foo")));
        assert!(!path_matches("src/foobar.rs", &normalize_prefix("src/foo")));
    }
}
