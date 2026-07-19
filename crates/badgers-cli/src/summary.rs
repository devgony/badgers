use std::fmt::Write;

use badgers_core::CoverageSnapshot;

pub fn render(snapshot: &CoverageSnapshot) -> String {
    let path_width = snapshot
        .files
        .iter()
        .map(|f| f.path.len())
        .chain(["File".len(), "TOTAL".len()])
        .max()
        .unwrap_or(5);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<path_width$}  {:>7}  {:>7}  {:>8}",
        "File", "Lines", "Covered", "Coverage"
    );
    for file in &snapshot.files {
        let _ = writeln!(
            out,
            "{:<path_width$}  {:>7}  {:>7}  {:>8}",
            file.path,
            file.executable_lines(),
            file.covered_lines(),
            fmt_pct(file.coverage_pct())
        );
    }
    let _ = writeln!(
        out,
        "{:<path_width$}  {:>7}  {:>7}  {:>8}",
        "TOTAL",
        snapshot.executable_lines(),
        snapshot.covered_lines(),
        fmt_pct(snapshot.coverage_pct())
    );
    out
}

fn fmt_pct(pct: Option<f64>) -> String {
    match pct {
        Some(p) => format!("{p:.2}%"),
        None => "n/a".to_string(),
    }
}
