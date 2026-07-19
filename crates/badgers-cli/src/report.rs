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

pub(crate) fn read_snapshot(path: &Path) -> Result<CoverageSnapshot> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read snapshot '{}'", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse snapshot '{}'", path.display()))
}

pub(crate) fn git_diff_output(repo_root: &Path, range: &str) -> Result<String> {
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
.notice { background: #fff8c5; border: 1px solid rgba(212,167,44,.4); border-radius: 8px;
          padding: 10px 14px; font-size: 13px; color: #4d2d00; margin-bottom: 12px; }
.tree { background: #fff; border: 1px solid #d0d7de; border-radius: 8px;
        overflow: hidden; font-size: 13px; }
.tree .row { display: grid; align-items: center;
             grid-template-columns: minmax(220px,1fr) 80px 80px 150px 110px 140px;
             border-bottom: 1px solid #eaeef2; }
.tree .cell { padding: 6px 12px; text-align: right; white-space: nowrap;
              overflow: hidden; text-overflow: ellipsis; }
.tree .cell.name { text-align: left; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
.tree .row.header { background: #f6f8fa; font-weight: 600; color: #57606a; }
.tree .row.header .cell { padding: 8px 12px; }
.tree summary.row { cursor: pointer; list-style: none; background: #f6f8fa; font-weight: 600; }
.tree summary.row::-webkit-details-marker { display: none; }
.tree summary .name::before { content: "▸ "; color: #57606a; }
.tree details[open] > summary .name::before { content: "▾ "; }
.tree summary.row:hover, .tree .row.file:hover { background: #eaeef2; }
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

fn coverage_cell_text(pct: Option<f64>, source: &Option<String>) -> String {
    match pct {
        Some(p) => format!("{p:.2}%"),
        None => match source {
            Some(text) if text.lines().next().is_none() => "empty file".to_string(),
            _ => "no executable lines".to_string(),
        },
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

#[derive(Default)]
struct DirNode {
    dirs: BTreeMap<String, DirNode>,
    files: Vec<usize>,
}

impl DirNode {
    fn insert(&mut self, parts: &[&str], idx: usize) {
        match parts {
            [_leaf] => self.files.push(idx),
            [dir, rest @ ..] => self
                .dirs
                .entry((*dir).to_string())
                .or_default()
                .insert(rest, idx),
            [] => {}
        }
    }
}

#[derive(Default, Clone, Copy)]
struct DirAgg {
    head_covered: u64,
    head_executable: u64,
    base_covered: u64,
    base_executable: u64,
    diff_covered: u32,
    diff_relevant: u32,
}

impl DirAgg {
    fn add_file(&mut self, delta: &FileDelta) {
        if let Some(c) = delta.head {
            self.head_covered += c.covered;
            self.head_executable += c.executable;
        }
        if let Some(c) = delta.base {
            self.base_covered += c.covered;
            self.base_executable += c.executable;
        }
        self.diff_covered += delta.diff.covered;
        self.diff_relevant += delta.diff.relevant;
    }

    fn merge(&mut self, other: DirAgg) {
        self.head_covered += other.head_covered;
        self.head_executable += other.head_executable;
        self.base_covered += other.base_covered;
        self.base_executable += other.base_executable;
        self.diff_covered += other.diff_covered;
        self.diff_relevant += other.diff_relevant;
    }

    fn delta_pct(&self, base_available: bool) -> Option<f64> {
        if !base_available {
            return None;
        }
        let head = badgers_core::coverage_pct(self.head_covered, self.head_executable)?;
        let base = badgers_core::coverage_pct(self.base_covered, self.base_executable)?;
        Some(head - base)
    }
}

fn aggregate(node: &DirNode, files: &[FileDelta]) -> DirAgg {
    let mut agg = DirAgg::default();
    for &idx in &node.files {
        agg.add_file(&files[idx]);
    }
    for child in node.dirs.values() {
        agg.merge(aggregate(child, files));
    }
    agg
}

fn diff_cell(covered: u32, relevant: u32) -> String {
    if relevant > 0 {
        format!(
            "{covered}/{relevant} ({})",
            fmt_pct(badgers_core::coverage_pct(
                u64::from(covered),
                u64::from(relevant)
            ))
        )
    } else {
        "–".to_string()
    }
}

fn indent(depth: usize) -> usize {
    12 + depth * 18
}

fn render_tree(node: &DirNode, depth: usize, comparison: &Comparison, out: &mut String) {
    for (name, child) in &node.dirs {
        // Compress chains of single-child directories (apps/x/src/x -> one row).
        let mut display = name.clone();
        let mut target = child;
        while target.files.is_empty() && target.dirs.len() == 1 {
            let (next_name, next) = target.dirs.iter().next().expect("len checked");
            display.push('/');
            display.push_str(next_name);
            target = next;
        }
        let agg = aggregate(target, &comparison.files);
        let coverage = if agg.head_executable == 0 {
            "–".to_string()
        } else {
            fmt_pct(badgers_core::coverage_pct(
                agg.head_covered,
                agg.head_executable,
            ))
        };
        let open = if depth == 0 { " open" } else { "" };
        let _ = writeln!(
            out,
            r#"<details class="dirnode"{open}><summary class="row dir">
<span class="cell name" style="padding-left:{pad}px">{display}/</span>
<span class="cell">{exec}</span><span class="cell">{cov}</span>
<span class="cell">{coverage}</span><span class="cell">{delta}</span>
<span class="cell">{diff}</span></summary>"#,
            pad = indent(depth),
            display = html_escape(&display),
            exec = agg.head_executable,
            cov = agg.head_covered,
            delta = delta_cell(agg.delta_pct(comparison.base_available)),
            diff = diff_cell(agg.diff_covered, agg.diff_relevant),
        );
        render_tree(target, depth + 1, comparison, out);
        out.push_str("</details>\n");
    }

    for &idx in &node.files {
        let delta = &comparison.files[idx];
        let file_name = delta.path.rsplit('/').next().unwrap_or(&delta.path);
        let name_cell = if delta.head.is_some() {
            format!(
                r#"<a href="{}">{}</a>"#,
                page_name(idx),
                html_escape(file_name)
            )
        } else {
            format!(
                r#"<span class="removed">{} (removed)</span>"#,
                html_escape(file_name)
            )
        };
        let coverage = match delta.head {
            Some(c) if c.executable == 0 => {
                r#"<span class="flat">no executable lines</span>"#.to_string()
            }
            Some(c) => fmt_pct(c.pct()),
            None => "n/a".to_string(),
        };
        let (covered, executable) = delta
            .head
            .map(|c| (c.covered, c.executable))
            .unwrap_or((0, 0));
        let _ = writeln!(
            out,
            r#"<div class="row file">
<span class="cell name" style="padding-left:{pad}px">{name_cell}</span>
<span class="cell">{executable}</span><span class="cell">{covered}</span>
<span class="cell">{coverage}</span><span class="cell">{delta_html}</span>
<span class="cell">{diff}</span></div>"#,
            pad = indent(depth) + 18,
            delta_html = delta_cell(delta.delta_pct()),
            diff = diff_cell(delta.diff.covered, delta.diff.relevant),
        );
    }
}

fn render_index(head: &CoverageSnapshot, comparison: &Comparison) -> String {
    let head_totals = comparison.head_totals();
    let diff_totals = comparison.diff_totals();

    let mut root = DirNode::default();
    for (idx, delta) in comparison.files.iter().enumerate() {
        let parts: Vec<&str> = delta.path.split('/').collect();
        root.insert(&parts, idx);
    }
    let mut rows = String::new();
    render_tree(&root, 0, comparison, &mut rows);

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
<div class="tree">
<div class="row header">
<span class="cell name">File</span><span class="cell">Lines</span>
<span class="cell">Covered</span><span class="cell">Coverage</span>
<span class="cell">Δ</span><span class="cell">Diff coverage</span></div>
{rows}
</div>
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
    let body = match &source {
        Some(text) if text.lines().next().is_none() => {
            r#"<div class="notice">This file is empty — there is nothing to measure.</div>"#
                .to_string()
        }
        Some(text) => {
            let mut rows = String::new();
            for (idx, raw) in text.lines().enumerate() {
                let line = (idx + 1) as u32;
                rows.push_str(&code_row(
                    line,
                    hits.get(&line).copied(),
                    is_changed(line),
                    raw,
                ));
            }
            let notice = if file.executable_lines() == 0 {
                r#"<div class="notice">No executable lines — this file is not measurable (comments/blank lines only).</div>"#
            } else {
                ""
            };
            format!("{notice}<table class=\"code\"><tbody>\n{rows}</tbody></table>")
        }
        None => {
            let mut rows = String::new();
            for lh in &file.line_hits {
                rows.push_str(&code_row(
                    lh.line,
                    Some(lh.hits),
                    is_changed(lh.line),
                    "(source not available)",
                ));
            }
            format!(
                "<div class=\"notice\">Source file not found in checkout — showing executable lines only.</div>\
                 <table class=\"code\"><tbody>\n{rows}</tbody></table>"
            )
        }
    };

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
{body}
</main></body></html>
"#,
        path = html_escape(&file.path),
        pct = coverage_cell_text(file.coverage_pct(), &source),
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
