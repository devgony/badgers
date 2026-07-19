use std::collections::{BTreeMap, BTreeSet};

use crate::{CoverageSnapshot, FileCoverage, coverage_pct};

/// Changed (added/modified) line numbers per repo-relative path, as produced
/// by parsing a unified git diff of base...head.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChangedLines(pub BTreeMap<String, BTreeSet<u32>>);

impl ChangedLines {
    pub fn for_path(&self, path: &str) -> Option<&BTreeSet<u32>> {
        self.0.get(path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub covered: u64,
    pub executable: u64,
}

impl Counts {
    fn of(file: &FileCoverage) -> Self {
        Self {
            covered: u64::from(file.covered_lines()),
            executable: u64::from(file.executable_lines()),
        }
    }

    pub fn pct(&self) -> Option<f64> {
        coverage_pct(self.covered, self.executable)
    }
}

/// Diff coverage for one file: changed executable lines and how many of
/// them are covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffCoverage {
    pub relevant: u32,
    pub covered: u32,
    pub uncovered_lines: Vec<u32>,
}

impl DiffCoverage {
    pub fn pct(&self) -> Option<f64> {
        coverage_pct(u64::from(self.covered), u64::from(self.relevant))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDelta {
    pub path: String,
    /// `None` when the file does not exist in the base snapshot (new file).
    pub base: Option<Counts>,
    /// `None` when the file was removed in head.
    pub head: Option<Counts>,
    pub diff: DiffCoverage,
}

impl FileDelta {
    /// Head pct minus base pct; `None` unless both sides are measurable.
    pub fn delta_pct(&self) -> Option<f64> {
        Some(self.head?.pct()? - self.base?.pct()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub base_available: bool,
    pub files: Vec<FileDelta>,
}

impl Comparison {
    pub fn head_totals(&self) -> Counts {
        accumulate(self.files.iter().filter_map(|f| f.head))
    }

    pub fn base_totals(&self) -> Counts {
        accumulate(self.files.iter().filter_map(|f| f.base))
    }

    pub fn delta_pct(&self) -> Option<f64> {
        if !self.base_available {
            return None;
        }
        Some(self.head_totals().pct()? - self.base_totals().pct()?)
    }

    pub fn diff_totals(&self) -> DiffCoverage {
        let mut total = DiffCoverage {
            relevant: 0,
            covered: 0,
            uncovered_lines: Vec::new(),
        };
        for file in &self.files {
            total.relevant += file.diff.relevant;
            total.covered += file.diff.covered;
        }
        total
    }
}

fn accumulate(counts: impl Iterator<Item = Counts>) -> Counts {
    counts.fold(
        Counts {
            covered: 0,
            executable: 0,
        },
        |acc, c| Counts {
            covered: acc.covered + c.covered,
            executable: acc.executable + c.executable,
        },
    )
}

pub fn compare(
    base: Option<&CoverageSnapshot>,
    head: &CoverageSnapshot,
    changed: &ChangedLines,
) -> Comparison {
    let base_files: BTreeMap<&str, &FileCoverage> = base
        .map(|snapshot| {
            snapshot
                .files
                .iter()
                .map(|f| (f.path.as_str(), f))
                .collect()
        })
        .unwrap_or_default();

    let mut files = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();

    for head_file in &head.files {
        seen.insert(head_file.path.as_str());
        files.push(FileDelta {
            path: head_file.path.clone(),
            base: base_files
                .get(head_file.path.as_str())
                .map(|f| Counts::of(f)),
            head: Some(Counts::of(head_file)),
            diff: diff_coverage(head_file, changed),
        });
    }

    for (path, base_file) in base_files {
        if !seen.contains(path) {
            files.push(FileDelta {
                path: path.to_string(),
                base: Some(Counts::of(base_file)),
                head: None,
                diff: DiffCoverage {
                    relevant: 0,
                    covered: 0,
                    uncovered_lines: Vec::new(),
                },
            });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Comparison {
        base_available: base.is_some(),
        files,
    }
}

fn diff_coverage(file: &FileCoverage, changed: &ChangedLines) -> DiffCoverage {
    let Some(changed_lines) = changed.for_path(&file.path) else {
        return DiffCoverage {
            relevant: 0,
            covered: 0,
            uncovered_lines: Vec::new(),
        };
    };
    let mut relevant = 0;
    let mut covered = 0;
    let mut uncovered_lines = Vec::new();
    for lh in &file.line_hits {
        if changed_lines.contains(&lh.line) {
            relevant += 1;
            if lh.hits > 0 {
                covered += 1;
            } else {
                uncovered_lines.push(lh.line);
            }
        }
    }
    DiffCoverage {
        relevant,
        covered,
        uncovered_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Language, LineHit, ToolVersions};

    fn snapshot(files: Vec<FileCoverage>) -> CoverageSnapshot {
        CoverageSnapshot::new(
            "o/r".into(),
            "sha".into(),
            None,
            None,
            "2026-07-19T00:00:00Z".into(),
            ToolVersions {
                badgers: "0.1.0".into(),
                cargo_llvm_cov: None,
                coverage_py: None,
            },
            files,
        )
    }

    fn file(path: &str, hits: &[(u32, u64)]) -> FileCoverage {
        FileCoverage::new(
            path.into(),
            Language::from_path(path),
            hits.iter()
                .map(|&(line, hits)| LineHit { line, hits })
                .collect(),
        )
    }

    fn changed(path: &str, lines: &[u32]) -> ChangedLines {
        let mut map = BTreeMap::new();
        map.insert(path.to_string(), lines.iter().copied().collect());
        ChangedLines(map)
    }

    #[test]
    fn computes_per_file_delta_and_diff_coverage() {
        let base = snapshot(vec![file("a.py", &[(1, 1), (2, 0)])]);
        let head = snapshot(vec![file("a.py", &[(1, 1), (2, 1), (3, 0)])]);
        let comparison = compare(Some(&base), &head, &changed("a.py", &[2, 3, 9]));

        assert_eq!(comparison.files.len(), 1);
        let delta = &comparison.files[0];
        assert_eq!(delta.base.unwrap().pct().unwrap(), 50.0);
        let head_pct = delta.head.unwrap().pct().unwrap();
        assert!((head_pct - 66.666).abs() < 0.01);
        assert!((delta.delta_pct().unwrap() - 16.666).abs() < 0.01);
        assert_eq!(delta.diff.relevant, 2);
        assert_eq!(delta.diff.covered, 1);
        assert_eq!(delta.diff.uncovered_lines, vec![3]);
    }

    #[test]
    fn handles_added_and_removed_files() {
        let base = snapshot(vec![file("gone.py", &[(1, 1)])]);
        let head = snapshot(vec![file("new.py", &[(1, 0)])]);
        let comparison = compare(Some(&base), &head, &ChangedLines::default());

        let gone = comparison
            .files
            .iter()
            .find(|f| f.path == "gone.py")
            .unwrap();
        assert!(gone.head.is_none());
        let new = comparison
            .files
            .iter()
            .find(|f| f.path == "new.py")
            .unwrap();
        assert!(new.base.is_none());
        assert_eq!(new.delta_pct(), None);
    }

    #[test]
    fn no_base_snapshot_yields_no_delta() {
        let head = snapshot(vec![file("a.py", &[(1, 1)])]);
        let comparison = compare(None, &head, &ChangedLines::default());
        assert!(!comparison.base_available);
        assert_eq!(comparison.delta_pct(), None);
        assert_eq!(comparison.head_totals().pct().unwrap(), 100.0);
    }
}
