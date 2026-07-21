use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use badge_rs_core::compare::{COMPARISON_SCHEMA_VERSION, Comparison, ComparisonDocument};
use clap::Args;

use crate::github_storage::{
    DEFAULT_STORAGE_BRANCH, DEFAULT_STORAGE_PREFIX, GithubReportLocation, checked_repo_path,
    clone_storage_branch, read_pointer, resolve_source_repo,
};

const MAX_COMPARISON_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Pull request number (defaults to the current branch's pull request)
    pub pr: Option<u64>,

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
}

pub fn run(args: &DiffArgs) -> Result<()> {
    let pr = match args.pr {
        Some(pr) => pr,
        None => current_pr()?,
    };
    ensure!(pr > 0, "pull request number must be greater than zero");

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
    let pointer = read_pointer(&location, pr, checkout.path())?;
    let document = read_comparison(
        &location,
        &pointer.commit_sha,
        pointer.comparison_key.as_deref(),
        checkout.path(),
    )?;

    print!("{}", render(pr, &document));
    Ok(())
}

fn current_pr() -> Result<u64> {
    let output = Command::new("gh")
        .args(["pr", "view", "--json", "number", "--jq", ".number"])
        .output()
        .context(
            "running gh pr view; pass a pull request number or install and authenticate GitHub CLI",
        )?;
    if !output.status.success() {
        bail!(
            "could not determine the current pull request: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let number =
        String::from_utf8(output.stdout).context("gh pr view returned non-UTF-8 output")?;
    number
        .trim()
        .parse()
        .context("gh pr view returned an invalid pull request number")
}

fn read_comparison(
    location: &GithubReportLocation,
    head_sha: &str,
    comparison_key: Option<&str>,
    checkout: &Path,
) -> Result<ComparisonDocument> {
    let key = comparison_key.context(
        "the latest PR report does not contain a coverage comparison; enable GitHub repository storage and rerun coverage",
    )?;
    let expected = location.comparison_path(head_sha)?;
    ensure!(
        key == expected,
        "PR pointer comparison path does not match commit {head_sha}"
    );

    let path = checked_repo_path(checkout, key)
        .with_context(|| format!("locating coverage comparison at {key}"))?;
    ensure!(path.is_file(), "stored coverage comparison is not a file");
    let file = std::fs::File::open(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut decoder =
        zstd::Decoder::new(file).with_context(|| format!("decompressing {}", path.display()))?;
    let mut data = Vec::new();
    decoder
        .by_ref()
        .take(MAX_COMPARISON_BYTES + 1)
        .read_to_end(&mut data)
        .with_context(|| format!("decompressing {}", path.display()))?;
    ensure!(
        data.len() as u64 <= MAX_COMPARISON_BYTES,
        "stored coverage comparison exceeds 64 MiB"
    );

    let document: ComparisonDocument =
        serde_json::from_slice(&data).with_context(|| format!("parsing {}", path.display()))?;
    ensure!(
        document.schema_version == COMPARISON_SCHEMA_VERSION,
        "unsupported comparison schema version {}",
        document.schema_version
    );
    ensure!(
        document.head_sha.eq_ignore_ascii_case(head_sha),
        "stored comparison head SHA {} does not match PR head {head_sha}",
        document.head_sha
    );
    Ok(document)
}

fn render(pr: u64, document: &ComparisonDocument) -> String {
    let comparison = &document.comparison;
    let uncovered: usize = comparison
        .files
        .iter()
        .map(|file| {
            let mut lines = file.diff.uncovered_lines.clone();
            lines.sort_unstable();
            lines.dedup();
            lines.len()
        })
        .sum();
    let noun = if uncovered == 1 { "line" } else { "lines" };
    let mut out = String::new();
    if uncovered == 0 {
        let _ = writeln!(out, "Coverage diff: no uncovered changed executable lines");
    } else {
        let _ = writeln!(
            out,
            "Coverage diff: {uncovered} uncovered changed executable {noun}"
        );
    }
    let short_sha = &document.head_sha[..document.head_sha.len().min(7)];
    let _ = writeln!(out, "PR: #{pr} @ {short_sha}");

    let totals = comparison.head_totals();
    let _ = writeln!(
        out,
        "Total coverage: {} ({})",
        format_pct(totals.pct()),
        format_delta(comparison)
    );
    let diff = comparison.diff_totals();
    let _ = writeln!(
        out,
        "Changed-line coverage: {} ({}/{})",
        format_pct(diff.pct()),
        diff.covered,
        diff.relevant
    );

    if uncovered > 0 {
        let mut files: Vec<_> = comparison
            .files
            .iter()
            .filter(|file| !file.diff.uncovered_lines.is_empty())
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        for file in files {
            let mut lines = file.diff.uncovered_lines.clone();
            lines.sort_unstable();
            lines.dedup();
            let ranges = line_ranges(&lines);
            let _ = writeln!(
                out,
                "{}:{} [changed-uncovered]",
                escape_path(&file.path),
                ranges.join(",")
            );
        }
    }
    out
}

fn format_pct(value: Option<f64>) -> String {
    value
        .map(|pct| format!("{pct:.2}%"))
        .unwrap_or_else(|| "n/a".into())
}

fn format_delta(comparison: &Comparison) -> String {
    if !comparison.base_available {
        return "no baseline".into();
    }
    comparison
        .delta_pct()
        .map(|delta| format!("{delta:+.2}pp"))
        .unwrap_or_else(|| "n/a".into())
}

fn line_ranges(lines: &[u32]) -> Vec<String> {
    let Some((&first, rest)) = lines.split_first() else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    let mut start = first;
    let mut end = first;
    for &line in rest {
        if line == end.saturating_add(1) {
            end = line;
        } else {
            ranges.push(format_range(start, end));
            start = line;
            end = line;
        }
    }
    ranges.push(format_range(start, end));
    ranges
}

fn format_range(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

fn escape_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        if character == '\\' || character.is_control() {
            escaped.extend(character.escape_default());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use badge_rs_core::compare::{Counts, DiffCoverage, FileDelta};

    use super::*;

    const HEAD_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn document() -> ComparisonDocument {
        ComparisonDocument {
            schema_version: COMPARISON_SCHEMA_VERSION,
            head_sha: HEAD_SHA.into(),
            base_sha: Some("89abcdef0123456789abcdef0123456789abcdef".into()),
            comparison: Comparison {
                base_available: true,
                files: vec![
                    FileDelta {
                        path: "src/store.rs".into(),
                        base: Some(Counts {
                            covered: 2,
                            executable: 2,
                        }),
                        head: Some(Counts {
                            covered: 1,
                            executable: 2,
                        }),
                        diff: DiffCoverage {
                            relevant: 1,
                            covered: 0,
                            uncovered_lines: vec![91],
                        },
                    },
                    FileDelta {
                        path: "src/parser.rs".into(),
                        base: Some(Counts {
                            covered: 1,
                            executable: 1,
                        }),
                        head: Some(Counts {
                            covered: 1,
                            executable: 3,
                        }),
                        diff: DiffCoverage {
                            relevant: 3,
                            covered: 1,
                            uncovered_lines: vec![48, 42, 47, 47],
                        },
                    },
                ],
            },
        }
    }

    #[test]
    fn renders_compact_deterministic_diff() {
        assert_eq!(
            render(547, &document()),
            "Coverage diff: 4 uncovered changed executable lines\n\
PR: #547 @ 0123456\n\
Total coverage: 40.00% (-60.00pp)\n\
Changed-line coverage: 25.00% (1/4)\n\
src/parser.rs:42,47-48 [changed-uncovered]\n\
src/store.rs:91 [changed-uncovered]\n"
        );
    }

    #[test]
    fn escapes_control_characters_in_paths() {
        assert_eq!(
            escape_path("src/line\nname\\file.rs"),
            "src/line\\nname\\\\file.rs"
        );
    }

    #[test]
    fn renders_no_uncovered_lines_without_a_baseline() {
        let mut document = document();
        document.comparison.base_available = false;
        document.comparison.files.clear();
        assert_eq!(
            render(7, &document),
            "Coverage diff: no uncovered changed executable lines\n\
PR: #7 @ 0123456\n\
Total coverage: n/a (no baseline)\n\
Changed-line coverage: n/a (0/0)\n"
        );
    }

    #[test]
    fn reads_and_validates_stored_comparison() {
        let temp = tempfile::tempdir().unwrap();
        let location = GithubReportLocation::new(
            "owner/project",
            "owner/project",
            DEFAULT_STORAGE_BRANCH,
            DEFAULT_STORAGE_PREFIX,
        )
        .unwrap();
        let key = location.comparison_path(HEAD_SHA).unwrap();
        let path = temp.path().join(&key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json = serde_json::to_vec(&document()).unwrap();
        std::fs::write(&path, zstd::encode_all(json.as_slice(), 3).unwrap()).unwrap();

        assert_eq!(
            read_comparison(&location, HEAD_SHA, Some(&key), temp.path()).unwrap(),
            document()
        );
        assert!(
            read_comparison(
                &location,
                HEAD_SHA,
                Some("badgers/repos/owner/project/commits/wrong/comparison.json.zst"),
                temp.path()
            )
            .is_err()
        );
    }
}
