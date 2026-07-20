use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use badge_rs_core::CoverageSnapshot;
use badge_rs_core::compare::{
    COMPARISON_SCHEMA_VERSION, ChangedLines, Comparison, ComparisonDocument, FileDelta, compare,
};
use badge_rs_core::coverage_pct;
use badge_rs_core::diff::parse_unified_diff;
use clap::Args;

use crate::report::{git_diff_output, read_snapshot};

#[derive(Args, Debug)]
pub struct MarkdownArgs {
    /// Head coverage snapshot JSON
    #[arg(long, value_name = "PATH")]
    pub head: PathBuf,

    /// Base coverage snapshot JSON to compare against
    #[arg(long, value_name = "PATH")]
    pub base: Option<PathBuf>,

    /// Git range for changed lines, e.g. "origin/main...HEAD"
    #[arg(long, value_name = "RANGE")]
    pub git_diff: Option<String>,

    /// Precomputed zero-context unified diff file (alternative to --git-diff)
    #[arg(long, value_name = "PATH", conflicts_with = "git_diff")]
    pub diff_file: Option<PathBuf>,

    /// Repository root for git diff
    #[arg(long, value_name = "PATH", default_value = ".")]
    pub repo_root: PathBuf,

    /// Base URL for source links, ending at `/blob/{sha}`
    #[arg(long)]
    pub source_url: Option<String>,

    /// Link to the pull request's Files changed view
    #[arg(long)]
    pub files_changed_url: Option<String>,

    /// Output Markdown file
    #[arg(
        short,
        long,
        value_name = "PATH",
        default_value = "coverage-summary.md"
    )]
    pub output: PathBuf,

    /// Optional machine-readable comparison JSON output
    #[arg(long, value_name = "PATH")]
    pub comparison_output: Option<PathBuf>,
}

pub fn run(args: &MarkdownArgs) -> Result<()> {
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
    if let Some(path) = &args.comparison_output {
        let document = ComparisonDocument {
            schema_version: COMPARISON_SCHEMA_VERSION,
            head_sha: head.commit_sha.clone(),
            base_sha: base.as_ref().map(|snapshot| snapshot.commit_sha.clone()),
            comparison: comparison.clone(),
        };
        fs::write(path, serde_json::to_vec_pretty(&document)?)
            .with_context(|| format!("failed to write '{}'", path.display()))?;
    }
    let source_url = args
        .source_url
        .clone()
        .or_else(|| default_source_url(&head));
    if let Some(url) = &source_url {
        validate_https_url(url, "source URL")?;
    }
    if let Some(url) = &args.files_changed_url {
        validate_https_url(url, "Files changed URL")?;
    }
    let markdown = render(
        &head,
        &comparison,
        source_url.as_deref(),
        args.files_changed_url.as_deref(),
    );
    fs::write(&args.output, markdown)
        .with_context(|| format!("failed to write '{}'", args.output.display()))?;
    println!("Markdown report written to {}", args.output.display());
    Ok(())
}

fn default_source_url(head: &CoverageSnapshot) -> Option<String> {
    if !valid_repo_slug(&head.repo) || !valid_commit_sha(&head.commit_sha) {
        return None;
    }
    Some(format!(
        "https://github.com/{}/blob/{}",
        head.repo, head.commit_sha
    ))
}

fn validate_https_url(url: &str, label: &str) -> Result<()> {
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

fn valid_repo_slug(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(owner), Some(name), None) if valid_part(owner) && valid_part(name)
    )
}

fn valid_commit_sha(sha: &str) -> bool {
    (7..=64).contains(&sha.len()) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
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
    fn add_file(&mut self, file: &FileDelta) {
        if let Some(counts) = file.head {
            self.head_covered += counts.covered;
            self.head_executable += counts.executable;
        }
        if let Some(counts) = file.base {
            self.base_covered += counts.covered;
            self.base_executable += counts.executable;
        }
        self.diff_covered += file.diff.covered;
        self.diff_relevant += file.diff.relevant;
    }

    fn merge(&mut self, other: Self) {
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
        let head = coverage_pct(self.head_covered, self.head_executable)?;
        let base = coverage_pct(self.base_covered, self.base_executable)?;
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

fn render(
    head: &CoverageSnapshot,
    comparison: &Comparison,
    source_url: Option<&str>,
    files_changed_url: Option<&str>,
) -> String {
    let mut out = String::new();
    let totals = comparison.head_totals();
    let diff = comparison.diff_totals();
    let _ = writeln!(out, "# 🦡 Badgers Coverage Report\n");
    let _ = writeln!(
        out,
        "**Head:** {}\n",
        code_path(&short_sha(&head.commit_sha))
    );
    if let Some(url) =
        files_changed_url.filter(|url| validate_https_url(url, "Files changed URL").is_ok())
    {
        let _ = writeln!(
            out,
            "**Pull request:** {}\n",
            html_link(url, "Files changed and annotations")
        );
    }
    let _ = writeln!(out, "| Scope | Coverage | Δ |");
    let _ = writeln!(out, "|---|---:|---:|");
    let _ = writeln!(
        out,
        "| **Total** | {} ({}/{}) | {} |",
        fmt_pct(totals.pct()),
        totals.covered,
        totals.executable,
        fmt_delta(comparison.delta_pct(), comparison.base_available)
    );
    let _ = writeln!(
        out,
        "| **Changed lines** | {} | — |\n",
        diff_cell(diff.covered, diff.relevant)
    );

    let mut root = DirNode::default();
    for (idx, file) in comparison.files.iter().enumerate() {
        root.insert(&file.path.split('/').collect::<Vec<_>>(), idx);
    }

    render_changed_files(comparison, source_url, &mut out);
    let _ = writeln!(out, "## Coverage by path\n");
    render_files(&root.files, comparison, source_url, &mut out);
    render_tree(&root, 0, comparison, source_url, &mut out);
    out
}

fn render_tree(
    node: &DirNode,
    depth: usize,
    comparison: &Comparison,
    source_url: Option<&str>,
    out: &mut String,
) {
    for (name, child) in &node.dirs {
        let agg = aggregate(child, &comparison.files);
        let open = if depth == 0 { " open" } else { "" };
        let indent = "&#x2003;".repeat(depth);
        let _ = writeln!(
            out,
            "<details{open}>\n<summary>{indent}📁 <strong>{}/</strong> — {} ({}/{}) · Δ {} · Diff {}</summary>\n",
            markdown_html_text(name),
            fmt_pct(coverage_pct(agg.head_covered, agg.head_executable)),
            agg.head_covered,
            agg.head_executable,
            fmt_delta(
                agg.delta_pct(comparison.base_available),
                comparison.base_available
            ),
            diff_cell(agg.diff_covered, agg.diff_relevant),
        );
        render_files(&child.files, comparison, source_url, out);
        render_tree(child, depth + 1, comparison, source_url, out);
        let _ = writeln!(out, "</details>\n");
    }
}

fn render_files(
    indices: &[usize],
    comparison: &Comparison,
    source_url: Option<&str>,
    out: &mut String,
) {
    if indices.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "| File | Lines | Covered | Coverage | Δ | Diff coverage |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|");
    for &idx in indices {
        let file = &comparison.files[idx];
        let (executable, covered, coverage) = match file.head {
            Some(head) => (
                head.executable.to_string(),
                head.covered.to_string(),
                fmt_pct(head.pct()),
            ),
            None => ("—".to_string(), "—".to_string(), "removed".to_string()),
        };
        let _ = writeln!(
            out,
            "| {} | {executable} | {covered} | {coverage} | {} | {} |",
            if file.head.is_some() {
                file_link(&file.path, source_url)
            } else {
                code_path(&file.path)
            },
            fmt_delta(file.delta_pct(), comparison.base_available),
            diff_cell(file.diff.covered, file.diff.relevant),
        );
    }
    let _ = writeln!(out);
}

fn render_changed_files(comparison: &Comparison, source_url: Option<&str>, out: &mut String) {
    let changed: Vec<&FileDelta> = comparison
        .files
        .iter()
        .filter(|file| file.diff.relevant > 0)
        .collect();
    let _ = writeln!(out, "## Changed executable lines\n");
    if changed.is_empty() {
        let _ = writeln!(out, "No measurable changed lines.\n");
        return;
    }
    for file in changed {
        let _ = writeln!(
            out,
            "<details{}>\n<summary>{} — {}</summary>\n",
            if file.diff.uncovered_lines.is_empty() {
                ""
            } else {
                " open"
            },
            file_link(&file.path, source_url),
            diff_cell(file.diff.covered, file.diff.relevant)
        );
        if file.diff.uncovered_lines.is_empty() {
            let _ = writeln!(out, "✅ All changed executable lines are covered.\n");
        } else {
            let links = line_ranges(&file.diff.uncovered_lines)
                .into_iter()
                .map(|(start, end)| line_link(&file.path, start, end, source_url))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "**Uncovered:** {links}\n");
        }
        let _ = writeln!(out, "</details>\n");
    }
}

fn file_link(path: &str, source_url: Option<&str>) -> String {
    match source_url.filter(|base| {
        validate_https_url(base, "source URL").is_ok() && valid_relative_source_path(path)
    }) {
        Some(base) => {
            let href = format!("{}/{}", base.trim_end_matches('/'), encode_path(path));
            format!(
                "<a href=\"{}\">{}</a>",
                html_attr_escape(&href),
                code_path(path)
            )
        }
        None => code_path(path),
    }
}

fn line_link(path: &str, start: u32, end: u32, source_url: Option<&str>) -> String {
    let label = if start == end {
        format!("L{start}")
    } else {
        format!("L{start}-L{end}")
    };
    match source_url.filter(|base| {
        validate_https_url(base, "source URL").is_ok() && valid_relative_source_path(path)
    }) {
        Some(base) => {
            let anchor_end = if start == end {
                String::new()
            } else {
                format!("-L{end}")
            };
            let href = format!(
                "{}/{}#L{start}{anchor_end}",
                base.trim_end_matches('/'),
                encode_path(path)
            );
            format!("<a href=\"{}\">{label}</a>", html_attr_escape(&href))
        }
        None => label,
    }
}

fn valid_relative_source_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn line_ranges(lines: &[u32]) -> Vec<(u32, u32)> {
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
            ranges.push((start, end));
            start = line;
            end = line;
        }
    }
    ranges.push((start, end));
    ranges
}

fn encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn code_path(path: &str) -> String {
    let visible = visible_path(path);
    format!("<code>{}</code>", markdown_html_text(&visible))
}

fn visible_path(value: &str) -> String {
    let mut visible = String::new();
    for ch in value.chars() {
        match ch {
            '\n' => visible.push_str("\\n"),
            '\r' => visible.push_str("\\r"),
            '\t' => visible.push_str("\\t"),
            ch if ch.is_control() => {
                let _ = write!(visible, "\\u{{{:X}}}", u32::from(ch));
            }
            ch => visible.push(ch),
        }
    }
    visible
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn html_attr_escape(value: &str) -> String {
    html_escape(value).replace('\'', "&#39;")
}

fn html_link(url: &str, label: &str) -> String {
    format!(
        "<a href=\"{}\">{}</a>",
        html_attr_escape(url),
        markdown_html_text(label)
    )
}

fn markdown_html_text(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '-' | ' ') {
            escaped.push(ch);
        } else if ch.is_ascii() {
            let _ = write!(escaped, "&#x{:X};", u32::from(ch));
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

fn fmt_pct(pct: Option<f64>) -> String {
    match pct {
        Some(value) => format!("{value:.2}%"),
        None => "n/a".to_string(),
    }
}

fn fmt_delta(delta: Option<f64>, base_available: bool) -> String {
    match delta {
        Some(value) if value >= 0.005 => format!("🟢 +{value:.2}%p"),
        Some(value) if value <= -0.005 => format!("🔴 {value:.2}%p"),
        Some(_) => "➖ ±0.00%p".to_string(),
        None if base_available => "n/a".to_string(),
        None => "n/a (no baseline)".to_string(),
    }
}

fn diff_cell(covered: u32, relevant: u32) -> String {
    if relevant == 0 {
        return "n/a".to_string();
    }
    format!(
        "{} ({covered}/{relevant})",
        fmt_pct(coverage_pct(u64::from(covered), u64::from(relevant)))
    )
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use badge_rs_core::ToolVersions;
    use badge_rs_core::compare::{Counts, DiffCoverage};

    use super::*;

    fn snapshot() -> CoverageSnapshot {
        CoverageSnapshot::new(
            "owner/repo".into(),
            "abcdef1234567890".into(),
            None,
            Some(7),
            "2026-07-19T00:00:00Z".into(),
            ToolVersions {
                badgers: "0.1.1".into(),
                cargo_llvm_cov: None,
                coverage_py: Some("7.6".into()),
            },
            vec![],
        )
    }

    #[test]
    fn renders_nested_tree_and_changed_line_links() {
        let comparison = Comparison {
            base_available: true,
            files: vec![FileDelta {
                path: "apps/api/src/app.py".into(),
                base: Some(Counts {
                    covered: 2,
                    executable: 2,
                }),
                head: Some(Counts {
                    covered: 2,
                    executable: 4,
                }),
                diff: DiffCoverage {
                    relevant: 3,
                    covered: 1,
                    uncovered_lines: vec![4, 5],
                },
            }],
        };
        let markdown = render(
            &snapshot(),
            &comparison,
            Some("https://github.com/owner/repo/blob/abcdef1"),
            Some("https://github.com/owner/repo/pull/7/files"),
        );
        assert!(markdown.contains("<summary>📁 <strong>apps/</strong>"));
        assert!(markdown.contains("<summary>&#x2003;📁 <strong>api/</strong>"));
        assert!(markdown.contains("<summary>&#x2003;&#x2003;📁 <strong>src/</strong>"));
        assert!(markdown.contains(
            "<a href=\"https://github.com/owner/repo/blob/abcdef1/apps/api/src/app.py\"><code>apps/api/src/app.py</code></a>"
        ));
        assert!(markdown.contains(
            "<a href=\"https://github.com/owner/repo/blob/abcdef1/apps/api/src/app.py#L4-L5\">L4-L5</a>"
        ));
        assert!(markdown.contains("50.00% (2/4)"));
        assert!(markdown.contains("33.33% (1/3)"));
        assert!(markdown.contains(
            "<a href=\"https://github.com/owner/repo/pull/7/files\">Files changed and annotations</a>"
        ));
        assert!(markdown.contains("<details open>\n<summary>"));
        assert!(
            markdown.find("## Changed executable lines").unwrap()
                < markdown.find("## Coverage by path").unwrap()
        );
    }

    #[test]
    fn opens_only_changed_files_with_uncovered_lines() {
        let comparison = Comparison {
            base_available: false,
            files: vec![
                FileDelta {
                    path: "pkg/uncovered.py".into(),
                    base: None,
                    head: Some(Counts {
                        covered: 1,
                        executable: 2,
                    }),
                    diff: DiffCoverage {
                        relevant: 2,
                        covered: 1,
                        uncovered_lines: vec![2],
                    },
                },
                FileDelta {
                    path: "pkg/covered.py".into(),
                    base: None,
                    head: Some(Counts {
                        covered: 2,
                        executable: 2,
                    }),
                    diff: DiffCoverage {
                        relevant: 2,
                        covered: 2,
                        uncovered_lines: vec![],
                    },
                },
            ],
        };
        let mut markdown = String::new();
        render_changed_files(&comparison, None, &mut markdown);

        assert!(markdown.contains(
            "<details open>\n<summary><code>pkg/uncovered.py</code> — 50.00% (1/2)</summary>"
        ));
        assert!(
            markdown.contains(
                "<details>\n<summary><code>pkg/covered.py</code> — 100.00% (2/2)</summary>"
            )
        );
        assert!(!markdown.contains("<details open>\n<summary><code>pkg/covered.py</code>"));
    }

    #[test]
    fn path_encoding_and_range_compression() {
        assert_eq!(encode_path("pkg/my file.py"), "pkg/my%20file.py");
        assert_eq!(line_ranges(&[1, 2, 4, 8, 9]), vec![(1, 2), (4, 4), (8, 9)]);
    }

    #[test]
    fn default_link_uses_snapshot_repo_and_commit() {
        assert_eq!(
            default_source_url(&snapshot()).as_deref(),
            Some("https://github.com/owner/repo/blob/abcdef1234567890")
        );
    }

    #[test]
    fn renders_hostile_legal_paths_as_safe_html() {
        let path = "pkg/a`b|c\n.py";
        let link = file_link(path, Some("https://github.com/owner/repo/blob/abcdef1"));
        assert_eq!(
            link,
            "<a href=\"https://github.com/owner/repo/blob/abcdef1/pkg/a%60b%7Cc%0A.py\"><code>pkg/a&#x60;b&#x7C;c&#x5C;n.py</code></a>"
        );
        assert_eq!(
            file_link(
                "../secret.py",
                Some("https://github.com/owner/repo/blob/abcdef1")
            ),
            "<code>../secret.py</code>"
        );
    }

    #[test]
    fn rejects_unsafe_source_urls_and_snapshot_metadata() {
        assert!(validate_https_url("javascript:alert(1)", "source URL").is_err());
        assert!(validate_https_url("https://user@example.com/repo", "source URL").is_err());
        assert!(validate_https_url("https://example.com/repo#fragment", "source URL").is_err());

        let mut invalid = snapshot();
        invalid.repo = "owner/repo/extra".into();
        invalid.commit_sha = "`\n<h1>🦡abcdef".into();
        assert_eq!(default_source_url(&invalid), None);
        assert_eq!(short_sha(&invalid.commit_sha), "`\n<h1>🦡");

        let markdown = render(
            &invalid,
            &Comparison {
                base_available: false,
                files: vec![],
            },
            None,
            None,
        );
        assert!(markdown.contains("**Head:** <code>&#x60;&#x5C;n&#x3C;h1&#x3E;🦡</code>"));
        assert!(!markdown.contains("\n<h1>"));
    }

    #[test]
    fn prevents_markdown_emphasis_inside_html_code() {
        assert_eq!(
            code_path("pkg/__init__.py"),
            "<code>pkg/&#x5F;&#x5F;init&#x5F;&#x5F;.py</code>"
        );
    }
}
