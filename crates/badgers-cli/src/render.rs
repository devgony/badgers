use std::fmt::Write as _;

use badge_rs_core::compare::{
    Comparison, ComparisonAnalysis, CoverageScopeChangeKind, CoverageScopeEntry, FileDelta,
};

pub(crate) const MAX_RENDERED_SCOPE_PATHS: usize = 100;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderOptions {
    heading: &'static str,
    uncovered_qualifier: &'static str,
    marker: &'static str,
    show_changed_line_coverage: bool,
}

impl RenderOptions {
    pub(crate) const REPO_WIDE: Self = Self {
        heading: "Coverage",
        uncovered_qualifier: "",
        marker: "uncovered",
        show_changed_line_coverage: false,
    };

    const DIFF: Self = Self {
        heading: "Coverage diff",
        uncovered_qualifier: "changed ",
        marker: "changed-uncovered",
        show_changed_line_coverage: true,
    };
}

pub(crate) fn render_comparison(context: &str, comparison: &Comparison) -> String {
    render_comparison_with_options(context, comparison, RenderOptions::DIFF)
}

pub(crate) fn render_comparison_analysis(context: &str, analysis: &ComparisonAnalysis) -> String {
    render_comparison_inner(
        context,
        &analysis.comparison,
        Some(analysis),
        RenderOptions::DIFF,
    )
}

pub(crate) fn render_comparison_with_options(
    context: &str,
    comparison: &Comparison,
    options: RenderOptions,
) -> String {
    render_comparison_inner(context, comparison, None, options)
}

fn render_comparison_inner(
    context: &str,
    comparison: &Comparison,
    analysis: Option<&ComparisonAnalysis>,
    options: RenderOptions,
) -> String {
    let uncovered = uncovered_count(comparison);
    let noun = if uncovered == 1 { "line" } else { "lines" };
    let mut out = String::new();
    if uncovered == 0 {
        let _ = writeln!(
            out,
            "{}: no uncovered {}executable lines",
            options.heading, options.uncovered_qualifier
        );
    } else {
        let _ = writeln!(
            out,
            "{}: {uncovered} uncovered {}executable {noun}",
            options.heading, options.uncovered_qualifier
        );
    }
    let _ = writeln!(out, "{context}");

    let totals = comparison.head_totals();
    let _ = writeln!(
        out,
        "Total coverage: {} ({})",
        format_pct(totals.pct()),
        format_delta(comparison, analysis)
    );
    if options.show_changed_line_coverage {
        let diff = comparison.diff_totals();
        let _ = writeln!(
            out,
            "Changed-line coverage: {} ({}/{})",
            format_pct(diff.pct()),
            diff.covered,
            diff.relevant
        );
    }

    if let Some(analysis) = analysis.filter(|analysis| analysis.scope_changed()) {
        let _ = writeln!(out, "Coverage scope changed; aggregate delta suppressed");
        let (entries, omitted) = bounded_scope_entries(analysis);
        for entry in entries {
            let label = match entry.kind {
                CoverageScopeChangeKind::Appeared => "appeared",
                CoverageScopeChangeKind::Disappeared => "disappeared",
            };
            let _ = writeln!(out, "{label}: {}", escape_path(entry.path));
        }
        render_omitted_scope_count(&mut out, omitted);
    }

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
                "{}:{} [{}]",
                escape_path(&file.path),
                ranges.join(","),
                options.marker
            );
        }
    }
    out
}

pub(crate) fn uncovered_count(comparison: &Comparison) -> usize {
    comparison.files.iter().map(file_uncovered_count).sum()
}

fn file_uncovered_count(file: &FileDelta) -> usize {
    let mut lines = file.diff.uncovered_lines.clone();
    lines.sort_unstable();
    lines.dedup();
    lines.len()
}

fn format_pct(value: Option<f64>) -> String {
    value
        .map(|pct| format!("{pct:.2}%"))
        .unwrap_or_else(|| "n/a".into())
}

fn format_delta(comparison: &Comparison, analysis: Option<&ComparisonAnalysis>) -> String {
    if !comparison.base_available {
        return "no baseline".into();
    }
    if analysis.is_some_and(ComparisonAnalysis::scope_changed) {
        return "coverage scope changed".into();
    }
    comparison
        .delta_pct()
        .map(|delta| format!("{delta:+.2}pp"))
        .unwrap_or_else(|| "n/a".into())
}

pub(crate) fn bounded_scope_entries(
    analysis: &ComparisonAnalysis,
) -> (Vec<CoverageScopeEntry<'_>>, usize) {
    let entries = analysis.affected_entries();
    let omitted = entries.len().saturating_sub(MAX_RENDERED_SCOPE_PATHS);
    (
        entries.into_iter().take(MAX_RENDERED_SCOPE_PATHS).collect(),
        omitted,
    )
}

pub(crate) fn render_omitted_scope_count(out: &mut String, omitted: usize) {
    if omitted > 0 {
        let _ = writeln!(out, "... and {omitted} more");
    }
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

pub(crate) fn escape_path(path: &str) -> String {
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
