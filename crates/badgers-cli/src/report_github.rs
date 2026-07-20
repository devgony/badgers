use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use badge_rs_core::compare::{ChangedLines, Comparison, compare};
use badge_rs_core::coverage_pct;
use badge_rs_core::diff::parse_unified_diff;
use badge_rs_github::{CheckAnnotation, CommentAction, GithubClient};
use clap::Args;

use crate::github_storage::{
    DEFAULT_STORAGE_BRANCH, DEFAULT_STORAGE_PREFIX, GithubReportLocation, html_escape,
};
use crate::report::{git_diff_output, git_path_prefix, read_snapshot};

#[derive(Args, Debug)]
pub struct GithubArgs {
    /// Head coverage snapshot JSON
    #[arg(long, value_name = "PATH")]
    pub head: PathBuf,

    /// Base coverage snapshot JSON to compare against
    #[arg(long, value_name = "PATH")]
    pub base: Option<PathBuf>,

    /// Git range for changed lines, e.g. "origin/main...HEAD" (runs git diff)
    #[arg(long, value_name = "RANGE")]
    pub git_diff: Option<String>,

    /// Repository root for git
    #[arg(long, value_name = "PATH", default_value = ".")]
    pub repo_root: PathBuf,

    /// Repository slug, e.g. "owner/repo" (comment target)
    #[arg(long)]
    pub repo: String,

    /// Pull request number to comment on
    #[arg(long)]
    pub pr: u64,

    /// Head commit SHA shown in the comment
    #[arg(long)]
    pub head_sha: Option<String>,

    /// Baseline description shown in the comment, e.g. "exact abc1234"
    #[arg(long)]
    pub baseline_label: Option<String>,

    /// Link to the full HTML report (artifact or hosted)
    #[arg(long)]
    pub report_url: Option<String>,

    /// Link to the durable detailed Markdown coverage report
    #[arg(long)]
    pub markdown_report_url: Option<String>,

    /// Link to the pull request's Files changed view and annotations
    #[arg(long)]
    pub files_changed_url: Option<String>,

    /// GitHub repository containing the durable HTML report branch
    #[arg(long, requires = "head_sha")]
    pub storage_repo: Option<String>,

    /// Dedicated branch containing durable reports
    #[arg(long, default_value = DEFAULT_STORAGE_BRANCH)]
    pub storage_branch: String,

    /// Path prefix inside the durable report branch
    #[arg(long, default_value = DEFAULT_STORAGE_PREFIX)]
    pub storage_prefix: String,

    /// Emit a warning instead of failing when the comment cannot be posted
    #[arg(long)]
    pub soft_fail: bool,

    /// Skip the marker-based pull request comment
    #[arg(long)]
    pub skip_comment: bool,

    /// Publish a GitHub check run with uncovered changed-line annotations
    #[arg(long)]
    pub check_annotations: bool,
}

pub fn run(args: &GithubArgs) -> Result<()> {
    validate_navigation_url(args.markdown_report_url.as_deref(), "Markdown report URL")?;
    validate_navigation_url(args.files_changed_url.as_deref(), "Files changed URL")?;
    validate_report_url(args.report_url.as_deref())?;
    stored_report_location(args)?;

    let head = read_snapshot(&args.head)?;
    let base = args.base.as_deref().map(read_snapshot).transpose()?;
    let changed = match &args.git_diff {
        Some(range) => parse_unified_diff(&git_diff_output(&args.repo_root, range)?),
        None => ChangedLines::default(),
    };
    let comparison = compare(base.as_ref(), &head, &changed);

    let marker = comment_marker(args)?;
    let body = render_comment(&marker, &comparison, args);

    let token = std::env::var("GITHUB_TOKEN").context("GITHUB_TOKEN is required")?;
    let client = GithubClient::with_base_url(args.repo.clone(), token, github_api_url()?);
    if !args.skip_comment {
        let current_head = args
            .head_sha
            .as_deref()
            .map(|expected| match client.pull_request_head_sha(args.pr) {
                Ok(current) => Ok(Some((expected, current))),
                Err(error) if args.soft_fail => {
                    println!("::warning::badgers could not verify the current PR head: {error}");
                    Ok(None)
                }
                Err(error) => Err(error),
            })
            .transpose()?;
        let should_publish = match current_head.flatten() {
            Some((expected, current)) if !same_commit_sha(expected, &current) => {
                println!(
                    "::notice::stale coverage run for {expected}; current PR head is {current}; comment update skipped"
                );
                false
            }
            Some(_) => true,
            None => args.head_sha.is_none(),
        };
        if should_publish {
            let legacy_marker_prefix = format!("<!-- badgers-report:{}:{}:", args.repo, args.pr);
            match client.upsert_pr_comment_with_aliases(
                args.pr,
                &marker,
                &[&legacy_marker_prefix],
                &body,
            ) {
                Ok(CommentAction::Created) => println!("comment created on PR #{}", args.pr),
                Ok(CommentAction::Updated) => println!("comment updated on PR #{}", args.pr),
                Err(e) if args.soft_fail => {
                    println!("::warning::badgers could not post the PR comment: {e}");
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    if args.check_annotations {
        let head_sha = args
            .head_sha
            .as_deref()
            .context("--head-sha is required with --check-annotations")?;
        let analyzed_sha = git_head_sha(&args.repo_root)?;
        if analyzed_sha != head_sha {
            let message = format!(
                "analyzed commit {analyzed_sha} does not match annotation head {head_sha}; check annotations skipped"
            );
            if args.soft_fail {
                println!("::warning::{message}");
            } else {
                bail!(message);
            }
        } else {
            let prefix = git_path_prefix(&args.repo_root)?;
            let (annotations, total_ranges) = build_annotations(&comparison, &prefix);
            let uncovered_lines: usize = comparison
                .files
                .iter()
                .map(|file| file.diff.uncovered_lines.len())
                .sum();
            let title = if annotations.is_empty() {
                "All changed executable lines are covered"
            } else {
                "Changed executable lines need coverage"
            };
            let truncation = if total_ranges > annotations.len() {
                format!(" Limited to the first {} ranges.", annotations.len())
            } else {
                String::new()
            };
            let summary = format!(
                "{uncovered_lines} uncovered changed executable lines across {total_ranges} annotation ranges.{truncation}"
            );
            match client.create_check_run(
                "Badgers diff coverage",
                head_sha,
                title,
                &summary,
                &annotations,
            ) {
                Ok(id) => println!(
                    "check run created: {id} ({} annotations)",
                    annotations.len()
                ),
                Err(e) if args.soft_fail => {
                    println!("::warning::badgers could not publish check annotations: {e}");
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}

fn git_head_sha(repo_root: &std::path::Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("failed to invoke git for analyzed commit SHA")?;
    if !output.status.success() {
        bail!(
            "`git rev-parse HEAD` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn github_api_url() -> Result<String> {
    let url =
        std::env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".to_string());
    let Some(rest) = url.strip_prefix("https://") else {
        bail!("GITHUB_API_URL must use https://");
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || url.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        bail!("GITHUB_API_URL contains unsafe components");
    }
    Ok(url)
}

fn validate_navigation_url(url: Option<&str>, label: &str) -> Result<()> {
    let Some(url) = url else {
        return Ok(());
    };
    let Some(rest) = url.strip_prefix("https://") else {
        bail!("{label} must use https://");
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || url.contains(['?', '#'])
        || url
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '"' | '<' | '>'))
    {
        bail!("{label} contains unsafe characters or components");
    }
    Ok(())
}

fn validate_report_url(url: Option<&str>) -> Result<()> {
    let Some(url) = url else {
        return Ok(());
    };
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .context("HTML report URL must use http:// or https://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || url
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '"' | '<' | '>'))
    {
        bail!("HTML report URL contains unsafe characters or components");
    }
    Ok(())
}

const MAX_CHECK_ANNOTATIONS: usize = 1_000;

fn build_annotations(comparison: &Comparison, path_prefix: &str) -> (Vec<CheckAnnotation>, usize) {
    let prefix = path_prefix.trim_matches('/');
    let all = comparison
        .files
        .iter()
        .flat_map(|file| {
            let path = if prefix.is_empty() {
                file.path.clone()
            } else {
                format!("{prefix}/{}", file.path)
            };
            line_number_ranges(&file.diff.uncovered_lines)
                .into_iter()
                .map(move |(start, end)| CheckAnnotation::warning(path.clone(), start, end))
        })
        .collect::<Vec<_>>();
    let total = all.len();
    (all.into_iter().take(MAX_CHECK_ANNOTATIONS).collect(), total)
}

fn render_comment(marker: &str, comparison: &Comparison, args: &GithubArgs) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{marker}");
    let _ = writeln!(out, "## 🦡 Badgers Coverage Report");
    let _ = writeln!(out);
    let _ = writeln!(out, "| | Coverage | Δ |");
    let _ = writeln!(out, "|---|---|---|");

    let head = comparison.head_totals();
    let _ = writeln!(
        out,
        "| **Total** | {} ({}/{}) | {} |",
        fmt_pct(coverage_pct(head.covered, head.executable)),
        head.covered,
        head.executable,
        fmt_delta(comparison.delta_pct(), comparison.base_available),
    );

    let diff = comparison.diff_totals();
    let diff_cell = if diff.relevant == 0 {
        "no measurable changed lines".to_string()
    } else {
        format!(
            "{} ({}/{})",
            fmt_pct(coverage_pct(
                u64::from(diff.covered),
                u64::from(diff.relevant)
            )),
            diff.covered,
            diff.relevant,
        )
    };
    let _ = writeln!(out, "| **Diff** | {diff_cell} | |");
    let _ = writeln!(out);

    let mut context_line = Vec::new();
    if let Some(label) = &args.baseline_label {
        context_line.push(format!("**Baseline**: {label}"));
    } else {
        context_line.push("**Baseline**: none".to_string());
    }
    if let Some(sha) = &args.head_sha {
        context_line.push(format!("**Head**: `{}`", short_sha(sha)));
    }
    let _ = writeln!(out, "{}", context_line.join(" · "));

    render_uncovered(&mut out, comparison);

    let mut links = Vec::new();
    if let Some(url) = args
        .markdown_report_url
        .as_deref()
        .filter(|url| validate_navigation_url(Some(url), "Markdown report URL").is_ok())
    {
        links.push(html_link(url, "Detailed coverage report"));
    }
    if let Some(url) = args
        .files_changed_url
        .as_deref()
        .filter(|url| validate_navigation_url(Some(url), "Files changed URL").is_ok())
    {
        links.push(html_link(url, "Files changed annotations"));
    }
    if let Some(url) = args
        .report_url
        .as_deref()
        .filter(|url| validate_report_url(Some(url)).is_ok())
    {
        links.push(html_link(url, "HTML report"));
    }
    if !links.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "**Reports:** {}", links.join(" · "));
    }
    if let (Some(location), Some(sha)) = (
        stored_report_location(args).ok().flatten(),
        args.head_sha.as_deref(),
    ) {
        let report_spec = location
            .report_spec(sha)
            .expect("head SHA is validated before rendering");
        let command = location.view_command(args.pr);
        let _ = writeln!(out);
        let _ = writeln!(out, "<details><summary>View stored HTML locally</summary>");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "**Stored report:** <code>{}</code>",
            html_escape(&report_spec)
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "<pre><code>{}</code></pre>", html_escape(&command));
        let _ = writeln!(out, "</details>");
    }
    out
}

fn stored_report_location(args: &GithubArgs) -> Result<Option<GithubReportLocation>> {
    args.storage_repo
        .as_deref()
        .map(|storage_repo| {
            GithubReportLocation::new(
                &args.repo,
                storage_repo,
                &args.storage_branch,
                &args.storage_prefix,
            )
        })
        .transpose()
}

const MAX_RANGES_PER_FILE: usize = 10;

fn render_uncovered(out: &mut String, comparison: &Comparison) {
    let files: Vec<_> = comparison
        .files
        .iter()
        .filter(|f| !f.diff.uncovered_lines.is_empty())
        .collect();
    if files.is_empty() {
        return;
    }
    let total: usize = files.iter().map(|f| f.diff.uncovered_lines.len()).sum();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "<details><summary>Uncovered changed lines ({total})</summary>"
    );
    let _ = writeln!(out);
    for file in files {
        let ranges = line_ranges(&file.diff.uncovered_lines);
        let shown: Vec<String> = ranges.iter().take(MAX_RANGES_PER_FILE).cloned().collect();
        let suffix = if ranges.len() > MAX_RANGES_PER_FILE {
            format!(" … and {} more", ranges.len() - MAX_RANGES_PER_FILE)
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "- {}: {}{}",
            markdown_code_path(&file.path),
            shown.join(", "),
            suffix
        );
    }
    let _ = writeln!(out, "</details>");
}

fn line_ranges(lines: &[u32]) -> Vec<String> {
    line_number_ranges(lines)
        .into_iter()
        .map(|(start, end)| fmt_range(start, end))
        .collect()
}

fn line_number_ranges(lines: &[u32]) -> Vec<(u32, u32)> {
    let mut ranges = Vec::new();
    let mut iter = lines.iter().copied();
    let Some(mut start) = iter.next() else {
        return ranges;
    };
    let mut end = start;
    for line in iter {
        if line == end.saturating_add(1) {
            end = line;
        } else {
            ranges.push((start, end));
            start = line;
            end = line;
        }
    }
    ranges.push((start, end));
    ranges
}

fn fmt_range(start: u32, end: u32) -> String {
    if start == end {
        format!("L{start}")
    } else {
        format!("L{start}-L{end}")
    }
}

fn markdown_code_path(path: &str) -> String {
    let mut escaped = String::new();
    for ch in path.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '-' | ' ') {
            escaped.push(ch);
        } else if ch.is_ascii() {
            let _ = write!(escaped, "&#x{:X};", u32::from(ch));
        } else {
            escaped.push(ch);
        }
    }
    format!("<code>{escaped}</code>")
}

fn html_link(url: &str, label: &str) -> String {
    let escaped = url
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    format!("<a href=\"{escaped}\">{label}</a>")
}

fn fmt_pct(pct: Option<f64>) -> String {
    match pct {
        Some(p) => format!("{p:.2}%"),
        None => "n/a".to_string(),
    }
}

fn fmt_delta(delta: Option<f64>, base_available: bool) -> String {
    match delta {
        Some(d) if d >= 0.005 => format!("🟢 +{d:.2}%p"),
        Some(d) if d <= -0.005 => format!("🔴 {d:.2}%p"),
        Some(_) => "➖ ±0.00%p".to_string(),
        None if base_available => "n/a".to_string(),
        None => "n/a (no baseline)".to_string(),
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn same_commit_sha(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn comment_marker(args: &GithubArgs) -> Result<String> {
    if args.head_sha.as_deref().is_some_and(|head_sha| {
        head_sha.len() != 40 || !head_sha.bytes().all(|b| b.is_ascii_hexdigit())
    }) {
        bail!("--head-sha must be exactly 40 ASCII hexadecimal characters");
    }
    Ok(format!("<!-- badgers-report:{}:{} -->", args.repo, args.pr))
}

#[cfg(test)]
mod tests {
    use badge_rs_core::compare::{Counts, DiffCoverage, FileDelta};

    use super::*;

    fn args() -> GithubArgs {
        GithubArgs {
            head: PathBuf::new(),
            base: None,
            git_diff: None,
            repo_root: PathBuf::new(),
            repo: "owner/repo".into(),
            pr: 5,
            head_sha: Some("0123456789abcdef0123456789abcdef01234567".into()),
            baseline_label: Some("exact abc1234".into()),
            report_url: Some("https://example.com/report".into()),
            markdown_report_url: Some("https://example.com/report.md".into()),
            files_changed_url: Some("https://github.com/owner/repo/pull/5/files".into()),
            storage_repo: None,
            storage_branch: DEFAULT_STORAGE_BRANCH.into(),
            storage_prefix: DEFAULT_STORAGE_PREFIX.into(),
            soft_fail: false,
            skip_comment: false,
            check_annotations: false,
        }
    }

    fn comparison() -> Comparison {
        Comparison {
            base_available: true,
            files: vec![FileDelta {
                path: "pkg/calc.py".into(),
                base: Some(Counts {
                    covered: 2,
                    executable: 2,
                }),
                head: Some(Counts {
                    covered: 3,
                    executable: 4,
                }),
                diff: DiffCoverage {
                    relevant: 3,
                    covered: 1,
                    uncovered_lines: vec![5, 6, 7, 12],
                },
            }],
        }
    }

    #[test]
    fn renders_full_comment() {
        let body = render_comment("<!-- m -->", &comparison(), &args());
        assert!(body.starts_with("<!-- m -->\n"));
        assert!(body.contains("| **Total** | 75.00% (3/4) | 🔴 -25.00%p |"));
        assert!(body.contains("| **Diff** | 33.33% (1/3) | |"));
        assert!(body.contains("**Baseline**: exact abc1234 · **Head**: `0123456`"));
        assert!(body.contains("Uncovered changed lines (4)"));
        assert!(body.contains("- <code>pkg/calc.py</code>: L5-L7, L12"));
        assert!(body.contains(
            "**Reports:** <a href=\"https://example.com/report.md\">Detailed coverage report</a> · <a href=\"https://github.com/owner/repo/pull/5/files\">Files changed annotations</a> · <a href=\"https://example.com/report\">HTML report</a>"
        ));
    }

    #[test]
    fn renders_exact_stored_report_path_and_view_command() {
        let mut a = args();
        a.storage_repo = Some("reports/archive".into());
        a.storage_branch = "coverage/reports".into();
        a.storage_prefix = "badgers/history".into();
        let body = render_comment("<!-- m -->", &comparison(), &a);
        assert!(body.contains(
            "<code>reports/archive@coverage/reports:badgers/history/repos/owner/repo/commits/0123456789abcdef0123456789abcdef01234567/html/index.html</code>"
        ));
        assert!(body.contains(
            "<pre><code>badgers view 5 --repo owner/repo --storage-repo reports/archive --storage-branch coverage/reports --storage-prefix badgers/history</code></pre>"
        ));
    }

    #[test]
    fn comment_marker_is_stable_across_head_pushes() {
        let mut a = args();
        let first = comment_marker(&a).unwrap();
        assert_eq!(first, "<!-- badgers-report:owner/repo:5 -->");
        assert_eq!(comment_marker(&a).unwrap(), first);

        a.head_sha = Some("ABCDEF0123456789ABCDEF0123456789ABCDEF01".into());
        assert_eq!(comment_marker(&a).unwrap(), first);
    }

    #[test]
    fn comment_marker_rejects_invalid_head_shas() {
        let mut a = args();
        for head_sha in [
            "abc123",
            "０１２３４５６７８９abcdef0123456789abcdef01234567",
            "0123456789abcdef0123456789abcdef0123456g",
            "0123456789abcdef0123456789abcdef0-->\nhi",
        ] {
            a.head_sha = Some(head_sha.into());
            assert!(comment_marker(&a).is_err(), "accepted {head_sha:?}");
        }

        a.head_sha = None;
        assert_eq!(
            comment_marker(&a).unwrap(),
            "<!-- badgers-report:owner/repo:5 -->"
        );
    }

    #[test]
    fn current_head_comparison_is_case_insensitive_and_rejects_stale_runs() {
        assert!(same_commit_sha(
            "ABCDEF0123456789ABCDEF0123456789ABCDEF01",
            "abcdef0123456789abcdef0123456789abcdef01"
        ));
        assert!(!same_commit_sha(
            "0123456789abcdef0123456789abcdef01234567",
            "abcdef0123456789abcdef0123456789abcdef01"
        ));
    }

    #[test]
    fn no_baseline_and_no_changed_lines() {
        let mut cmp = comparison();
        cmp.base_available = false;
        cmp.files[0].base = None;
        cmp.files[0].diff = DiffCoverage {
            relevant: 0,
            covered: 0,
            uncovered_lines: vec![],
        };
        let mut a = args();
        a.baseline_label = None;
        a.report_url = None;
        a.markdown_report_url = None;
        a.files_changed_url = None;
        let body = render_comment("<!-- m -->", &cmp, &a);
        assert!(body.contains("n/a (no baseline)"));
        assert!(body.contains("no measurable changed lines"));
        assert!(body.contains("**Baseline**: none"));
        assert!(!body.contains("<details>"));
        assert!(!body.contains("HTML report"));
    }

    #[test]
    fn caps_ranges_per_file() {
        let mut cmp = comparison();
        cmp.files[0].diff.uncovered_lines = (0..30).map(|i| i * 2 + 1).collect();
        let body = render_comment("<!-- m -->", &cmp, &args());
        assert!(body.contains("… and 20 more"));
    }

    #[test]
    fn builds_repo_relative_annotations_from_contiguous_ranges() {
        let (annotations, total) = build_annotations(&comparison(), "examples/python-sample/");
        assert_eq!(total, 2);
        assert_eq!(annotations.len(), 2);
        assert_eq!(annotations[0].path, "examples/python-sample/pkg/calc.py");
        assert_eq!((annotations[0].start_line, annotations[0].end_line), (5, 7));
        assert_eq!(
            (annotations[1].start_line, annotations[1].end_line),
            (12, 12)
        );
        assert_eq!(annotations[0].annotation_level, "warning");
    }

    #[test]
    fn escapes_markdown_in_comment_paths() {
        assert_eq!(
            markdown_code_path("pkg/a`b_[x].py"),
            "<code>pkg/a&#x60;b&#x5F;&#x5B;x&#x5D;.py</code>"
        );
    }

    #[test]
    fn caps_check_annotation_ranges() {
        let mut cmp = comparison();
        cmp.files[0].diff.uncovered_lines = (0..=MAX_CHECK_ANNOTATIONS)
            .map(|index| u32::try_from(index * 2 + 1).unwrap())
            .collect();
        let (annotations, total) = build_annotations(&cmp, "");
        assert_eq!(annotations.len(), MAX_CHECK_ANNOTATIONS);
        assert_eq!(total, MAX_CHECK_ANNOTATIONS + 1);
    }

    #[test]
    fn rejects_unsafe_navigation_urls() {
        assert!(validate_navigation_url(Some("http://example.com"), "report URL").is_err());
        assert!(
            validate_navigation_url(Some("https://user@example.com/report"), "report URL").is_err()
        );
        assert!(
            validate_navigation_url(Some("https://example.com/report#fragment"), "report URL")
                .is_err()
        );
    }

    #[test]
    fn preserves_legacy_html_report_url_compatibility_and_escapes_links() {
        let mut a = args();
        a.markdown_report_url = None;
        a.files_changed_url = None;
        a.report_url = Some("http://example.com/report?view=full&tab=files#summary".into());
        assert!(validate_report_url(a.report_url.as_deref()).is_ok());

        let body = render_comment("<!-- m -->", &comparison(), &a);
        assert!(body.contains(
            "<a href=\"http://example.com/report?view=full&amp;tab=files#summary\">HTML report</a>"
        ));
        assert!(validate_report_url(Some("javascript:alert(1)")).is_err());
        assert!(validate_report_url(Some("https://user@example.com/report")).is_err());
    }
}
