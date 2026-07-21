use std::io::Read as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use badge_rs_core::compare::{COMPARISON_SCHEMA_VERSION, ComparisonDocument};
use clap::Args;

use crate::github_storage::{
    DEFAULT_STORAGE_BRANCH, DEFAULT_STORAGE_PREFIX, GithubReportLocation, checked_repo_path,
    clone_storage_branch, read_pointer, resolve_source_repo,
};
use crate::render::render_comparison;

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
    let short_sha = &document.head_sha[..document.head_sha.len().min(7)];
    render_comparison(&format!("PR: #{pr} @ {short_sha}"), &document.comparison)
}

#[cfg(test)]
mod tests {
    use badge_rs_core::compare::{Comparison, Counts, DiffCoverage, FileDelta};

    use super::*;
    use crate::render::escape_path;

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
