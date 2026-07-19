use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use badgers_core::compare::{ChangedLines, Comparison, FileDelta, compare};
use badgers_core::diff::parse_unified_diff;
use badgers_core::{CoverageSnapshot, FileCoverage};
use clap::Args;

#[derive(Args, Debug)]
pub struct HtmlArgs {
    /// Head coverage snapshot JSON
    #[arg(long, value_name = "PATH")]
    pub head: PathBuf,

    /// Base coverage snapshot JSON to compare against
    #[arg(long, value_name = "PATH")]
    pub base: Option<PathBuf>,

    /// Git range for changed lines, e.g. "origin/main...HEAD" (runs git diff)
    #[arg(long, value_name = "RANGE")]
    pub git_diff: Option<String>,

    /// Precomputed unified diff file (alternative to --git-diff)
    #[arg(long, value_name = "PATH", conflicts_with = "git_diff")]
    pub diff_file: Option<PathBuf>,

    /// Repository root for git and source lookup
    #[arg(long, value_name = "PATH", default_value = ".")]
    pub repo_root: PathBuf,

    /// Output directory for the HTML report
    #[arg(short, long, value_name = "DIR", default_value = "coverage-report")]
    pub output: PathBuf,
}

pub fn run(args: &HtmlArgs) -> Result<()> {
    let head = read_snapshot(&args.head)?;
    let base = args.base.as_deref().map(read_snapshot).transpose()?;

    let changed = if let Some(range) = &args.git_diff {
        parse_unified_diff(&git_diff_output(&args.repo_root, range)?)
    } else if let Some(path) = &args.diff_file {
        parse_unified_diff(
            &fs::read_to_string(path)
                .with_context(|| format!("failed to read diff file '{}'", path.display()))?,
        )
    } else {
        ChangedLines::default()
    };

    let comparison = compare(base.as_ref(), &head, &changed);
    write_report(&head, &comparison, &changed, &args.repo_root, &args.output)?;
    println!(
        "HTML report written to {}",
        args.output.join("index.html").display()
    );
    Ok(())
}

fn read_snapshot(path: &Path) -> Result<CoverageSnapshot> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read snapshot '{}'", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse snapshot '{}'", path.display()))
}

fn git_diff_output(repo_root: &Path, range: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "diff",
            "--no-color",
            "--relative",
            "--unified=0",
            "--diff-filter=ACMR",
            range,
        ])
        .output()
        .context("failed to invoke git")?;
    if !output.status.success() {
        bail!(
            "`git diff {range}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write_report(
    head: &CoverageSnapshot,
    comparison: &Comparison,
    changed: &ChangedLines,
    repo_root: &Path,
    out_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create '{}'", out_dir.display()))?;

    let head_files: BTreeMap<&str, &FileCoverage> =
        head.files.iter().map(|f| (f.path.as_str(), f)).collect();

    fs::write(out_dir.join("index.html"), render_index(head, comparison))?;

    for (idx, delta) in comparison.files.iter().enumerate() {
        let Some(file) = head_files.get(delta.path.as_str()) else {
            continue;
        };
        let page = render_file_page(file, delta, changed, repo_root);
        fs::write(out_dir.join(page_name(idx)), page)?;
    }
    Ok(())
}

fn page_name(idx: usize) -> String {
    format!("file-{idx}.html")
}

const STYLE: &str = r#"
:root { color-scheme: light; }
* { box-sizing: border-box; }
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
       margin: 0; background: #f6f8fa; color: #1f2328; }
header { background: #24292f; color: #fff; padding: 16px 24px; }
header h1 { margin: 0 0 4px; font-size: 18px; }
header .meta { font-size: 12px; color: #d0d7de; }
main { max-width: 1100px; margin: 24px auto; padding: 0 16px; }
.cards { display: flex; gap: 12px; margin-bottom: 20px; flex-wrap: wrap; }
.card { background: #fff; border: 1px solid #d0d7de; border-radius: 8px;
        padding: 12px 20px; min-width: 160px; }
.card .label { font-size: 12px; color: #57606a; }
.card .value { font-size: 22px; font-weight: 600; }
.card .sub { font-size: 12px; color: #57606a; }
.up { color: #1a7f37; } .down { color: #cf222e; } .flat { color: #57606a; }
table { border-collapse: collapse; width: 100%; background: #fff;
        border: 1px solid #d0d7de; border-radius: 8px; overflow: hidden; }
th, td { padding: 8px 12px; text-align: right; font-size: 13px;
         border-bottom: 1px solid #eaeef2; }
th { background: #f6f8fa; color: #57606a; font-weight: 600; }
td.path, th.path { text-align: left; font-family: ui-monospace, monospace; }
tr:hover td { background: #f6f8fa; }
a { color: #0969da; text-decoration: none; }
a:hover { text-decoration: underline; }
.removed { color: #8c959f; }
.legend { font-size: 12px; color: #57606a; margin: 12px 0; }
.legend span { padding: 2px 8px; border-radius: 4px; margin-right: 8px; }
table.code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
table.code td { padding: 0 8px; font-size: 12.5px; line-height: 1.7;
                border-bottom: none; white-space: pre; text-align: left; }
td.ln { text-align: right !important; color: #8c959f; width: 1%;
        user-select: none; border-right: 1px solid #eaeef2; }
td.ln a { color: inherit; }
td.hits { text-align: right !important; color: #57606a; width: 1%;
          border-right: 1px solid #eaeef2; }
td.chg { width: 1%; color: #0969da; font-weight: 700; user-select: none; }
tr.cov { background: #e6ffec; }
tr.miss { background: #ffebe9; }
tr.cov.changed { background: #abf2bc; }
tr.miss.changed { background: #ffb3ad; }
tr.changed td.chg::before { content: "±"; }
tr:target td { outline: 2px solid #0969da; }
.uncovered-links { margin: 8px 0 16px; font-size: 13px; }
.uncovered-links a { margin-right: 6px; }
"#;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn fmt_pct(pct: Option<f64>) -> String {
    match pct {
        Some(p) => format!("{p:.2}%"),
        None => "n/a".to_string(),
    }
}

fn delta_cell(delta: Option<f64>) -> String {
    match delta {
        Some(d) if d > 0.005 => format!(r#"<span class="up">+{d:.2}%p</span>"#),
        Some(d) if d < -0.005 => format!(r#"<span class="down">{d:.2}%p</span>"#),
        Some(_) => r#"<span class="flat">±0.00%p</span>"#.to_string(),
        None => r#"<span class="flat">n/a</span>"#.to_string(),
    }
}

fn short_sha(sha: &str) -> &str {
    if sha.len() >= 7 { &sha[..7] } else { sha }
}

fn render_index(head: &CoverageSnapshot, comparison: &Comparison) -> String {
    let head_totals = comparison.head_totals();
    let diff_totals = comparison.diff_totals();

    let mut rows = String::new();
    for (idx, delta) in comparison.files.iter().enumerate() {
        let path = html_escape(&delta.path);
        let name_cell = if delta.head.is_some() {
            format!(r#"<a href="{}">{path}</a>"#, page_name(idx))
        } else {
            format!(r#"<span class="removed">{path} (removed)</span>"#)
        };
        let head_pct = fmt_pct(delta.head.and_then(|c| c.pct()));
        let diff_cell = if delta.diff.relevant > 0 {
            format!(
                "{}/{} ({})",
                delta.diff.covered,
                delta.diff.relevant,
                fmt_pct(delta.diff.pct())
            )
        } else {
            "–".to_string()
        };
        let (covered, executable) = delta
            .head
            .map(|c| (c.covered, c.executable))
            .unwrap_or((0, 0));
        let _ = writeln!(
            rows,
            "<tr><td class=\"path\">{name_cell}</td><td>{executable}</td><td>{covered}</td>\
             <td>{head_pct}</td><td>{}</td><td>{diff_cell}</td></tr>",
            delta_cell(delta.delta_pct()),
        );
    }

    let base_note = if comparison.base_available {
        "vs base snapshot"
    } else {
        "no base snapshot"
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>Badgers Coverage Report</title><style>{STYLE}</style></head>
<body>
<header><h1>🦡 Badgers Coverage Report</h1>
<div class="meta">{repo} @ {sha} · generated {generated}</div></header>
<main>
<div class="cards">
<div class="card"><div class="label">Total line coverage</div>
<div class="value">{total_pct}</div>
<div class="sub">{covered}/{executable} lines · {delta} {base_note}</div></div>
<div class="card"><div class="label">Diff coverage (changed lines)</div>
<div class="value">{diff_pct}</div>
<div class="sub">{diff_covered}/{diff_relevant} changed executable lines</div></div>
</div>
<table>
<thead><tr><th class="path">File</th><th>Lines</th><th>Covered</th>
<th>Coverage</th><th>Δ</th><th>Diff coverage</th></tr></thead>
<tbody>
{rows}
</tbody></table>
</main></body></html>
"#,
        repo = html_escape(&head.repo),
        sha = short_sha(&head.commit_sha),
        generated = html_escape(&head.generated_at),
        total_pct = fmt_pct(head_totals.pct()),
        covered = head_totals.covered,
        executable = head_totals.executable,
        delta = delta_cell(comparison.delta_pct()),
        diff_pct = fmt_pct(diff_totals.pct()),
        diff_covered = diff_totals.covered,
        diff_relevant = diff_totals.relevant,
    )
}

fn render_file_page(
    file: &FileCoverage,
    delta: &FileDelta,
    changed: &ChangedLines,
    repo_root: &Path,
) -> String {
    let hits: BTreeMap<u32, u64> = file.line_hits.iter().map(|lh| (lh.line, lh.hits)).collect();
    let changed_lines = changed.for_path(&file.path);
    let is_changed = |line: u32| changed_lines.is_some_and(|set| set.contains(&line));

    let source = fs::read_to_string(repo_root.join(&file.path)).ok();
    let mut rows = String::new();
    match &source {
        Some(text) => {
            for (idx, raw) in text.lines().enumerate() {
                let line = (idx + 1) as u32;
                rows.push_str(&code_row(
                    line,
                    hits.get(&line).copied(),
                    is_changed(line),
                    raw,
                ));
            }
        }
        None => {
            for lh in &file.line_hits {
                rows.push_str(&code_row(
                    lh.line,
                    Some(lh.hits),
                    is_changed(lh.line),
                    "(source not available)",
                ));
            }
        }
    }

    let uncovered_links = if delta.diff.uncovered_lines.is_empty() {
        String::new()
    } else {
        let links: Vec<String> = delta
            .diff
            .uncovered_lines
            .iter()
            .map(|l| format!(r##"<a href="#L{l}">L{l}</a>"##))
            .collect();
        format!(
            r#"<div class="uncovered-links">Uncovered changed lines: {}</div>"#,
            links.join(" ")
        )
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>{path} - Badgers Coverage</title><style>{STYLE}</style></head>
<body>
<header><h1>{path}</h1>
<div class="meta"><a href="index.html" style="color:#d0d7de">← index</a> ·
coverage {pct} · {delta_txt} · diff coverage {diff_pct} ({diff_covered}/{diff_relevant})</div></header>
<main>
<div class="legend">
<span style="background:#e6ffec">covered</span>
<span style="background:#ffebe9">uncovered</span>
<span style="background:#abf2bc">covered + changed</span>
<span style="background:#ffb3ad">uncovered + changed</span>
<span>± = line changed in this PR</span>
</div>
{uncovered_links}
<table class="code"><tbody>
{rows}
</tbody></table>
</main></body></html>
"#,
        path = html_escape(&file.path),
        pct = fmt_pct(file.coverage_pct()),
        delta_txt = delta_cell(delta.delta_pct()),
        diff_pct = fmt_pct(delta.diff.pct()),
        diff_covered = delta.diff.covered,
        diff_relevant = delta.diff.relevant,
    )
}

fn code_row(line: u32, hits: Option<u64>, changed: bool, source: &str) -> String {
    let mut classes: Vec<&str> = Vec::new();
    match hits {
        Some(h) if h > 0 => classes.push("cov"),
        Some(_) => classes.push("miss"),
        None => {}
    }
    if changed {
        classes.push("changed");
    }
    let class_attr = if classes.is_empty() {
        String::new()
    } else {
        format!(" class=\"{}\"", classes.join(" "))
    };
    let hits_txt = hits.map(|h| h.to_string()).unwrap_or_default();
    format!(
        "<tr id=\"L{line}\"{class_attr}><td class=\"ln\"><a href=\"#L{line}\">{line}</a></td>\
         <td class=\"hits\">{hits_txt}</td><td class=\"chg\"></td><td>{}</td></tr>\n",
        html_escape(source)
    )
}
