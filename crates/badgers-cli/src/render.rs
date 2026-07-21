use std::fmt::Write as _;

use badge_rs_core::compare::{Comparison, FileDelta};

pub(crate) fn render_comparison(context: &str, comparison: &Comparison) -> String {
    let uncovered = uncovered_count(comparison);
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
    let _ = writeln!(out, "{context}");

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
