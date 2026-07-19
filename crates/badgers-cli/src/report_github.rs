use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use badge_rs_core::compare::{ChangedLines, Comparison, compare};
use badge_rs_core::coverage_pct;
use badge_rs_core::diff::parse_unified_diff;
use badge_rs_github::{CommentAction, GithubClient};
use clap::Args;

use crate::report::{git_diff_output, read_snapshot};

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

    /// Emit a warning instead of failing when the comment cannot be posted
    #[arg(long)]
    pub soft_fail: bool,
}

pub fn run(args: &GithubArgs) -> Result<()> {
    let head = read_snapshot(&args.head)?;
    let base = args.base.as_deref().map(read_snapshot).transpose()?;
    let changed = match &args.git_diff {
        Some(range) => parse_unified_diff(&git_diff_output(&args.repo_root, range)?),
        None => ChangedLines::default(),
    };
    let comparison = compare(base.as_ref(), &head, &changed);

    let marker = format!("<!-- badgers-report:{}:{} -->", args.repo, args.pr);
    let body = render_comment(&marker, &comparison, args);

    let token = std::env::var("GITHUB_TOKEN").context("GITHUB_TOKEN is required")?;
    let client = GithubClient::new(args.repo.clone(), token);
    match client.upsert_pr_comment(args.pr, &marker, &body) {
        Ok(CommentAction::Created) => println!("comment created on PR #{}", args.pr),
        Ok(CommentAction::Updated) => println!("comment updated on PR #{}", args.pr),
        Err(e) if args.soft_fail => {
            println!("::warning::badgers could not post the PR comment: {e}");
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
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

    if let Some(url) = &args.report_url {
        let _ = writeln!(out);
        let _ = writeln!(out, "**HTML report**: {url}");
    }
    out
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
        let _ = writeln!(out, "- `{}`: {}{}", file.path, shown.join(", "), suffix);
    }
    let _ = writeln!(out, "</details>");
}

fn line_ranges(lines: &[u32]) -> Vec<String> {
    let mut ranges = Vec::new();
    let mut iter = lines.iter().copied();
    let Some(mut start) = iter.next() else {
        return ranges;
    };
    let mut end = start;
    for line in iter {
        if line == end + 1 {
            end = line;
        } else {
            ranges.push(fmt_range(start, end));
            start = line;
            end = line;
        }
    }
    ranges.push(fmt_range(start, end));
    ranges
}

fn fmt_range(start: u32, end: u32) -> String {
    if start == end {
        format!("L{start}")
    } else {
        format!("L{start}-L{end}")
    }
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

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
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
            head_sha: Some("def5678901234".into()),
            baseline_label: Some("exact abc1234".into()),
            report_url: Some("https://example.com/report".into()),
            soft_fail: false,
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
        assert!(body.contains("**Baseline**: exact abc1234 · **Head**: `def5678`"));
        assert!(body.contains("Uncovered changed lines (4)"));
        assert!(body.contains("- `pkg/calc.py`: L5-L7, L12"));
        assert!(body.contains("**HTML report**: https://example.com/report"));
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
}
