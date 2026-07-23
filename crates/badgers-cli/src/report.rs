use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use anyhow::{Context, Result, bail, ensure};
use badge_rs_core::compare::{
    ChangedLines, ComparisonAnalysis, CoverageScopeChangeKind, FileDelta, compare,
    compare_with_source_trees,
};
use badge_rs_core::diff::parse_unified_diff;
use badge_rs_core::{CoverageSnapshot, FileCoverage};
use clap::Args;

use crate::render::bounded_scope_entries;

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

    let comparison = compare_for_report(
        base.as_ref(),
        &head,
        &changed,
        &args.repo_root,
        args.git_diff.is_some(),
    )?;
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
            sanitize_git_stderr(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn git_tree_files(repo_root: &Path, commit: &str) -> Result<BTreeSet<String>> {
    const MAX_STDOUT_BYTES: usize = 64 * 1024 * 1024;
    const MAX_STDERR_BYTES: usize = 64 * 1024;

    ensure!(
        commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "git tree commit ID must be exactly 40 ASCII hexadecimal characters"
    );
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            "--end-of-options",
            commit,
            "--",
            ".",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to invoke git for source tree")?;
    let stdout = child
        .stdout
        .take()
        .context("failed to capture git tree stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture git tree stderr")?;
    let stdout_reader =
        thread::spawn(move || read_bounded(stdout, MAX_STDOUT_BYTES, "git tree stdout"));
    let stderr_reader =
        thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES, "git tree stderr"));
    let status = child.wait().context("failed to wait for git source tree")?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("git tree stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("git tree stderr reader panicked"))??;

    if !status.success() {
        bail!(
            "`git ls-tree -r --name-only {commit} -- .` failed: {}",
            sanitize_git_stderr(&stderr)
        );
    }

    Ok(stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::str::from_utf8(path).ok())
        .map(normalize_tree_path)
        .collect())
}

fn read_bounded(mut reader: impl std::io::Read, limit: usize, label: &str) -> Result<Vec<u8>> {
    let read_limit = u64::try_from(limit)
        .context("bounded read limit does not fit in u64")?
        .saturating_add(1);
    let mut data = Vec::new();
    reader
        .by_ref()
        .take(read_limit)
        .read_to_end(&mut data)
        .with_context(|| format!("failed to read {label}"))?;
    ensure!(data.len() <= limit, "{label} exceeds {limit} bytes");
    Ok(data)
}

fn sanitize_git_stderr(stderr: &[u8]) -> String {
    const MAX_CHARS: usize = 512;

    let text = String::from_utf8_lossy(stderr);
    let mut sanitized = String::new();
    let mut length = 0;
    let mut truncated = false;
    for character in text.chars() {
        let escaped: String = character.escape_default().collect();
        let escaped_length = escaped.chars().count();
        if length + escaped_length > MAX_CHARS {
            truncated = true;
            break;
        }
        sanitized.push_str(&escaped);
        length += escaped_length;
    }
    if truncated {
        sanitized.push('…');
    }
    sanitized
}

fn normalize_tree_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

pub(crate) fn compare_for_report(
    base: Option<&CoverageSnapshot>,
    head: &CoverageSnapshot,
    changed: &ChangedLines,
    repo_root: &Path,
    git_backed: bool,
) -> Result<ComparisonAnalysis> {
    let Some(base) = base.filter(|_| git_backed) else {
        return Ok(compare(base, head, changed).into());
    };
    let base_tree = git_tree_files(repo_root, &base.commit_sha)?;
    let head_tree = git_tree_files(repo_root, &head.commit_sha)?;
    Ok(compare_with_source_trees(
        Some(base),
        head,
        changed,
        Some(&base_tree),
        Some(&head_tree),
    ))
}

pub(crate) fn git_path_prefix(repo_root: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "--show-prefix"])
        .output()
        .context("failed to invoke git for repository path prefix")?;
    if !output.status.success() {
        bail!(
            "`git rev-parse --show-prefix` failed: {}",
            sanitize_git_stderr(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_report(
    head: &CoverageSnapshot,
    comparison: &ComparisonAnalysis,
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
        let head = badge_rs_core::coverage_pct(self.head_covered, self.head_executable)?;
        let base = badge_rs_core::coverage_pct(self.base_covered, self.base_executable)?;
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
            fmt_pct(badge_rs_core::coverage_pct(
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

fn render_tree(node: &DirNode, depth: usize, comparison: &ComparisonAnalysis, out: &mut String) {
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
            fmt_pct(badge_rs_core::coverage_pct(
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
            delta =
                delta_cell(agg.delta_pct(comparison.base_available && !comparison.scope_changed())),
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

fn render_index(head: &CoverageSnapshot, comparison: &ComparisonAnalysis) -> String {
    let head_totals = comparison.head_totals();
    let diff_totals = comparison.diff_totals();

    let mut root = DirNode::default();
    for (idx, delta) in comparison.files.iter().enumerate() {
        let parts: Vec<&str> = delta.path.split('/').collect();
        root.insert(&parts, idx);
    }
    let mut rows = String::new();
    render_tree(&root, 0, comparison, &mut rows);

    let base_note = if comparison.scope_changed() {
        "coverage scope changed"
    } else if comparison.base_available {
        "vs base snapshot"
    } else {
        "no base snapshot"
    };

    let scope_notice = render_scope_notice(comparison);

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
{scope_notice}
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

fn render_scope_notice(comparison: &ComparisonAnalysis) -> String {
    if !comparison.scope_changed() {
        return String::new();
    }
    let mut items = String::new();
    let (entries, omitted) = bounded_scope_entries(comparison);
    for entry in entries {
        let label = match entry.kind {
            CoverageScopeChangeKind::Appeared => "Appeared in coverage",
            CoverageScopeChangeKind::Disappeared => "Disappeared from coverage",
        };
        let _ = write!(
            items,
            "<li>{label}: <code>{}</code></li>",
            html_escape(&crate::render::escape_path(entry.path))
        );
    }
    if omitted > 0 {
        let _ = write!(items, "<li>... and {omitted} more</li>");
    }
    format!(
        "<div class=\"notice\"><strong>Coverage scope changed.</strong> Aggregate deltas are suppressed because the measured file set changed.<ul>{items}</ul></div>"
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::process::Command;

    use badge_rs_core::compare::{Comparison, Counts, CoverageScopeChange, DiffCoverage};
    use badge_rs_core::{Language, LineHit, ToolVersions};

    use super::*;

    #[test]
    fn html_explains_scope_change_and_suppresses_aggregate_deltas() {
        let snapshot = CoverageSnapshot::new(
            "owner/repo".into(),
            "abcdef1234567890".into(),
            None,
            None,
            "2026-07-23T00:00:00Z".into(),
            ToolVersions {
                badgers: "1.2.0".into(),
                cargo_llvm_cov: None,
                coverage_py: None,
                flutter: None,
            },
            vec![],
        );
        let comparison = ComparisonAnalysis {
            comparison: Comparison {
                base_available: true,
                files: vec![FileDelta {
                    path: "pkg/steady.py".into(),
                    base: Some(Counts {
                        covered: 1,
                        executable: 2,
                    }),
                    head: Some(Counts {
                        covered: 2,
                        executable: 2,
                    }),
                    diff: DiffCoverage {
                        relevant: 1,
                        covered: 1,
                        uncovered_lines: vec![],
                    },
                }],
            },
            scope_change: CoverageScopeChange {
                appeared: vec!["pkg/appeared.py".into()],
                disappeared: vec!["pkg/disappeared.py".into()],
            },
        };

        let html = render_index(&snapshot, &comparison);

        assert!(html.contains("Coverage scope changed."));
        assert!(html.contains("Aggregate deltas are suppressed"));
        assert!(html.contains("pkg/appeared.py"));
        assert!(html.contains("pkg/disappeared.py"));
        assert!(html.contains("<span class=\"flat\">n/a</span> coverage scope changed"));
        assert!(html.contains("<span class=\"up\">+50.00%p</span>"));
    }

    #[test]
    fn html_scope_paths_are_escaped_and_bounded() {
        let mut analysis: ComparisonAnalysis = Comparison {
            base_available: true,
            files: vec![],
        }
        .into();
        analysis.scope_change.appeared = (0..=100)
            .map(|index| format!("scope/{index:03}.py"))
            .collect();
        analysis.scope_change.appeared[0] = "scope/\ncontrol.py".into();

        let html = render_scope_notice(&analysis);

        assert!(html.contains("scope/\\ncontrol.py"));
        assert!(!html.contains("scope/\ncontrol.py"));
        assert!(html.contains("... and 1 more"));
        assert!(!html.contains("scope/100.py"));
    }

    #[test]
    fn normalizes_git_tree_paths_relative_to_repo_root() {
        assert_eq!(normalize_tree_path("./pkg\\module.py"), "pkg/module.py");
    }

    #[test]
    fn rejects_invalid_tree_commit_ids_and_sanitizes_git_errors() {
        let error = git_tree_files(Path::new("does-not-matter"), "--help\n::error::bad")
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "git tree commit ID must be exactly 40 ASCII hexadecimal characters"
        );

        assert_eq!(
            sanitize_git_stderr(b"fatal\r\n::error::injected"),
            "fatal\\r\\n::error::injected"
        );
        let sanitized = sanitize_git_stderr(&vec![b'x'; 600]);
        assert_eq!(sanitized.chars().count(), 513);
        assert!(sanitized.ends_with('…'));
    }

    #[test]
    fn bounded_reader_accepts_limit_and_rejects_limit_plus_one() {
        assert_eq!(
            read_bounded(Cursor::new(b"1234"), 4, "test output").unwrap(),
            b"1234"
        );
        let error = read_bounded(Cursor::new(b"12345"), 4, "test output")
            .unwrap_err()
            .to_string();
        assert_eq!(error, "test output exceeds 4 bytes");
    }

    #[cfg(unix)]
    #[test]
    fn git_backed_comparison_detects_appeared_path_from_real_commit_trees() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let nested_root = repo.join("nested");
        std::fs::create_dir_all(nested_root.join("src")).unwrap();

        git(&repo, &["init"]);
        git(&repo, &["config", "--local", "user.name", "Badgers Test"]);
        git(
            &repo,
            &["config", "--local", "user.email", "badgers@example.invalid"],
        );
        std::fs::write(nested_root.join("src/appeared.py"), "value = 1\n").unwrap();
        std::fs::write(nested_root.join("src/steady.py"), "value = 1\n").unwrap();
        git(&repo, &["add", "."]);
        let blob = git_with_stdin(&repo, &["hash-object", "-w", "--stdin"], b"unrelated\n");
        let non_utf8_path = OsString::from_vec(vec![
            b'n', b'e', b's', b't', b'e', b'd', b'/', b's', b'r', b'c', b'/', 0xff, b'.', b'b',
            b'i', b'n',
        ]);
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["update-index", "--add", "--cacheinfo", "100644", &blob])
            .arg(non_utf8_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "adding non-UTF-8 index path failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        git(&repo, &["commit", "-m", "base"]);
        let base_sha = git(&repo, &["rev-parse", "HEAD"]);

        std::fs::write(nested_root.join("src/steady.py"), "value = 2\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "head"]);
        let head_sha = git(&repo, &["rev-parse", "HEAD"]);

        assert_eq!(
            git_tree_files(&nested_root, &base_sha).unwrap(),
            BTreeSet::from(["src/appeared.py".to_string(), "src/steady.py".to_string(),])
        );

        let base = test_snapshot(base_sha, vec![test_file("src/steady.py", &[(1, 0)])]);
        let head = test_snapshot(
            head_sha,
            vec![
                test_file("src/appeared.py", &[(1, 1)]),
                test_file("src/steady.py", &[(1, 1)]),
            ],
        );
        let comparison = compare_for_report(
            Some(&base),
            &head,
            &ChangedLines::default(),
            &nested_root,
            true,
        )
        .unwrap();

        assert_eq!(comparison.scope_change.appeared, vec!["src/appeared.py"]);
        assert!(comparison.scope_change.disappeared.is_empty());
        assert_eq!(comparison.delta_pct(), None);
        assert_eq!(
            comparison
                .files
                .iter()
                .find(|file| file.path == "src/steady.py")
                .unwrap()
                .delta_pct(),
            Some(100.0)
        );
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn git_with_stdin(repo: &Path, args: &[&str], input: &[u8]) -> String {
        use std::io::Write as _;
        use std::process::Stdio;

        let mut child = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn test_snapshot(commit_sha: String, files: Vec<FileCoverage>) -> CoverageSnapshot {
        CoverageSnapshot::new(
            "owner/repo".into(),
            commit_sha,
            None,
            None,
            "2026-07-23T00:00:00Z".into(),
            ToolVersions {
                badgers: "1.2.0".into(),
                cargo_llvm_cov: None,
                coverage_py: None,
                flutter: None,
            },
            files,
        )
    }

    fn test_file(path: &str, hits: &[(u32, u64)]) -> FileCoverage {
        FileCoverage::new(
            path.into(),
            Language::from_path(path),
            hits.iter()
                .map(|&(line, hits)| LineHit { line, hits })
                .collect(),
        )
    }
}
