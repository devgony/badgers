use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{CoverageSnapshot, FileCoverage, coverage_pct};

pub const COMPARISON_SCHEMA_VERSION: u32 = 2;

pub fn is_supported_comparison_schema_version(version: u32) -> bool {
    (1..=COMPARISON_SCHEMA_VERSION).contains(&version)
}

/// Changed (added/modified) line numbers per repo-relative path, as produced
/// by parsing a unified git diff of base...head.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChangedLines(pub BTreeMap<String, BTreeSet<u32>>);

impl ChangedLines {
    pub fn for_path(&self, path: &str) -> Option<&BTreeSet<u32>> {
        self.0.get(path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Branch outcome counts for one side of a file comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchCounts {
    pub covered: u64,
    pub total: u64,
}

impl BranchCounts {
    /// `None` when the file carries no branch data (older snapshots or
    /// line-only coverage tools).
    fn of(file: &FileCoverage) -> Option<Self> {
        (file.total_branches() > 0).then(|| Self {
            covered: u64::from(file.covered_branches()),
            total: u64::from(file.total_branches()),
        })
    }

    pub fn pct(&self) -> Option<f64> {
        coverage_pct(self.covered, self.total)
    }
}

/// Diff coverage for one file: changed executable lines and how many of
/// them are covered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Branch diff coverage for one file: branches sitting on changed executable
/// lines and how many of them were taken.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BranchDiffCoverage {
    pub relevant: u32,
    pub covered: u32,
    /// Changed lines that executed but still have at least one branch that
    /// was never taken.
    pub partial_lines: Vec<u32>,
}

impl BranchDiffCoverage {
    pub fn is_empty(&self) -> bool {
        self.relevant == 0 && self.partial_lines.is_empty()
    }

    pub fn pct(&self) -> Option<f64> {
        coverage_pct(u64::from(self.covered), u64::from(self.relevant))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDelta {
    pub path: String,
    /// `None` when the file does not exist in the base snapshot (new file).
    pub base: Option<Counts>,
    /// `None` when the file was removed in head.
    pub head: Option<Counts>,
    pub diff: DiffCoverage,
    /// Additive fields; absent in comparisons written before branch support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branches: Option<BranchCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_branches: Option<BranchCounts>,
    #[serde(default, skip_serializing_if = "BranchDiffCoverage::is_empty")]
    pub branch_diff: BranchDiffCoverage,
}

impl FileDelta {
    /// Head pct minus base pct; `None` unless both sides are measurable.
    pub fn delta_pct(&self) -> Option<f64> {
        Some(self.head?.pct()? - self.base?.pct()?)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CoverageScopeChange {
    /// Files newly included in coverage despite already existing in the base tree.
    pub appeared: Vec<String>,
    /// Files omitted from head coverage despite still existing in the head tree.
    pub disappeared: Vec<String>,
}

impl CoverageScopeChange {
    pub fn is_empty(&self) -> bool {
        self.appeared.is_empty() && self.disappeared.is_empty()
    }

    pub fn affected_entries(&self) -> Vec<CoverageScopeEntry<'_>> {
        let appeared: BTreeSet<&str> = self.appeared.iter().map(String::as_str).collect();
        let disappeared: BTreeSet<&str> = self.disappeared.iter().map(String::as_str).collect();
        appeared
            .into_iter()
            .map(|path| CoverageScopeEntry {
                kind: CoverageScopeChangeKind::Appeared,
                path,
            })
            .chain(disappeared.into_iter().map(|path| CoverageScopeEntry {
                kind: CoverageScopeChangeKind::Disappeared,
                path,
            }))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageScopeChangeKind {
    Appeared,
    Disappeared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverageScopeEntry<'a> {
    pub kind: CoverageScopeChangeKind,
    pub path: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub base_available: bool,
    pub files: Vec<FileDelta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonAnalysis {
    #[serde(flatten)]
    pub comparison: Comparison,
    #[serde(default, skip_serializing_if = "CoverageScopeChange::is_empty")]
    pub scope_change: CoverageScopeChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonDocument {
    pub schema_version: u32,
    pub head_sha: String,
    pub base_sha: Option<String>,
    pub comparison: Comparison,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonAnalysisDocument {
    pub schema_version: u32,
    pub head_sha: String,
    pub base_sha: Option<String>,
    pub comparison: ComparisonAnalysis,
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

    /// `None` when no head file carries branch data.
    pub fn head_branch_totals(&self) -> Option<BranchCounts> {
        accumulate_branches(self.files.iter().filter_map(|f| f.head_branches))
    }

    /// `None` when no base file carries branch data.
    pub fn base_branch_totals(&self) -> Option<BranchCounts> {
        accumulate_branches(self.files.iter().filter_map(|f| f.base_branches))
    }

    /// `None` without a baseline or when either side lacks branch data.
    pub fn branch_delta_pct(&self) -> Option<f64> {
        if !self.base_available {
            return None;
        }
        Some(self.head_branch_totals()?.pct()? - self.base_branch_totals()?.pct()?)
    }

    pub fn branch_diff_totals(&self) -> BranchDiffCoverage {
        let mut total = BranchDiffCoverage::default();
        for file in &self.files {
            total.relevant += file.branch_diff.relevant;
            total.covered += file.branch_diff.covered;
        }
        total
    }
}

impl ComparisonAnalysis {
    pub fn new(comparison: Comparison) -> Self {
        Self {
            comparison,
            scope_change: CoverageScopeChange::default(),
        }
    }

    pub fn head_totals(&self) -> Counts {
        self.comparison.head_totals()
    }

    pub fn base_totals(&self) -> Counts {
        self.comparison.base_totals()
    }

    pub fn delta_pct(&self) -> Option<f64> {
        if self.scope_changed() {
            return None;
        }
        self.comparison.delta_pct()
    }

    pub fn diff_totals(&self) -> DiffCoverage {
        self.comparison.diff_totals()
    }

    pub fn branch_delta_pct(&self) -> Option<f64> {
        if self.scope_changed() {
            return None;
        }
        self.comparison.branch_delta_pct()
    }

    pub fn scope_changed(&self) -> bool {
        !self.scope_change.is_empty()
    }

    pub fn affected_entries(&self) -> Vec<CoverageScopeEntry<'_>> {
        self.scope_change.affected_entries()
    }
}

impl From<Comparison> for ComparisonAnalysis {
    fn from(comparison: Comparison) -> Self {
        Self::new(comparison)
    }
}

impl std::ops::Deref for ComparisonAnalysis {
    type Target = Comparison;

    fn deref(&self) -> &Self::Target {
        &self.comparison
    }
}

impl std::ops::DerefMut for ComparisonAnalysis {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.comparison
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

fn accumulate_branches(counts: impl Iterator<Item = BranchCounts>) -> Option<BranchCounts> {
    counts.fold(None, |acc, c| {
        let acc = acc.unwrap_or(BranchCounts {
            covered: 0,
            total: 0,
        });
        Some(BranchCounts {
            covered: acc.covered + c.covered,
            total: acc.total + c.total,
        })
    })
}

pub fn compare(
    base: Option<&CoverageSnapshot>,
    head: &CoverageSnapshot,
    changed: &ChangedLines,
) -> Comparison {
    build_comparison(base, head, changed)
}

/// Compares snapshots and detects coverage-scope drift when both corresponding
/// Git tree file sets are available. Tree checks are ignored without a base
/// snapshot or when either tree is unavailable.
pub fn compare_with_source_trees(
    base: Option<&CoverageSnapshot>,
    head: &CoverageSnapshot,
    changed: &ChangedLines,
    base_tree: Option<&BTreeSet<String>>,
    head_tree: Option<&BTreeSet<String>>,
) -> ComparisonAnalysis {
    let comparison = build_comparison(base, head, changed);
    let scope_change = match (base, base_tree, head_tree) {
        (Some(base), Some(base_tree), Some(head_tree)) => {
            detect_scope_change(base, head, base_tree, head_tree)
        }
        _ => CoverageScopeChange::default(),
    };
    ComparisonAnalysis {
        comparison,
        scope_change,
    }
}

fn build_comparison(
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
        let base_file = base_files.get(head_file.path.as_str());
        files.push(FileDelta {
            path: head_file.path.clone(),
            base: base_file.map(|f| Counts::of(f)),
            head: Some(Counts::of(head_file)),
            diff: diff_coverage(head_file, changed),
            base_branches: base_file.and_then(|f| BranchCounts::of(f)),
            head_branches: BranchCounts::of(head_file),
            branch_diff: branch_diff_coverage(head_file, changed),
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
                base_branches: BranchCounts::of(base_file),
                head_branches: None,
                branch_diff: BranchDiffCoverage::default(),
            });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Comparison {
        base_available: base.is_some(),
        files,
    }
}

fn detect_scope_change(
    base: &CoverageSnapshot,
    head: &CoverageSnapshot,
    base_tree: &BTreeSet<String>,
    head_tree: &BTreeSet<String>,
) -> CoverageScopeChange {
    let base_snapshot: BTreeSet<&str> = base.files.iter().map(|file| file.path.as_str()).collect();
    let head_snapshot: BTreeSet<&str> = head.files.iter().map(|file| file.path.as_str()).collect();

    let appeared = head_snapshot
        .difference(&base_snapshot)
        .filter(|path| base_tree.contains(**path) && head_tree.contains(**path))
        .map(|path| (*path).to_string())
        .collect();
    let disappeared = base_snapshot
        .difference(&head_snapshot)
        .filter(|path| base_tree.contains(**path) && head_tree.contains(**path))
        .map(|path| (*path).to_string())
        .collect();

    CoverageScopeChange {
        appeared,
        disappeared,
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

/// Branches on changed executable lines. `partial_lines` lists changed lines
/// that executed but still miss at least one branch outcome; lines that never
/// executed are already reported as uncovered lines.
fn branch_diff_coverage(file: &FileCoverage, changed: &ChangedLines) -> BranchDiffCoverage {
    let Some(changed_lines) = changed.for_path(&file.path) else {
        return BranchDiffCoverage::default();
    };
    if file.branches.is_empty() {
        return BranchDiffCoverage::default();
    }
    let executed: BTreeSet<u32> = file
        .line_hits
        .iter()
        .filter(|lh| lh.hits > 0)
        .map(|lh| lh.line)
        .collect();
    let mut totals = BranchDiffCoverage::default();
    let mut partial: BTreeSet<u32> = BTreeSet::new();
    for branch in &file.branches {
        if !changed_lines.contains(&branch.line) {
            continue;
        }
        totals.relevant += 1;
        if branch.is_covered() {
            totals.covered += 1;
        } else if executed.contains(&branch.line) {
            partial.insert(branch.line);
        }
    }
    totals.partial_lines = partial.into_iter().collect();
    totals
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

    fn branch(line: u32, branch: &str, taken: Option<u64>) -> crate::BranchHit {
        crate::BranchHit {
            line,
            block: "0".to_string(),
            branch: branch.to_string(),
            taken,
        }
    }

    fn file_with_branches(
        path: &str,
        hits: &[(u32, u64)],
        branches: Vec<crate::BranchHit>,
    ) -> FileCoverage {
        FileCoverage::detailed(
            path.into(),
            Language::from_path(path),
            hits.iter()
                .map(|&(line, hits)| LineHit { line, hits })
                .collect(),
            branches,
            Vec::new(),
        )
    }

    #[test]
    fn computes_branch_delta_and_changed_branch_coverage() {
        let base = snapshot(vec![file_with_branches(
            "a.py",
            &[(1, 1), (2, 1)],
            vec![branch(2, "0", Some(1)), branch(2, "1", Some(0))],
        )]);
        let head = snapshot(vec![file_with_branches(
            "a.py",
            &[(1, 1), (2, 1), (3, 1)],
            vec![
                branch(2, "0", Some(1)),
                branch(2, "1", Some(1)),
                branch(3, "0", Some(1)),
                branch(3, "1", Some(0)),
            ],
        )]);
        let comparison = compare(Some(&base), &head, &changed("a.py", &[2, 3]));

        let delta = &comparison.files[0];
        assert_eq!(
            delta.base_branches,
            Some(BranchCounts {
                covered: 1,
                total: 2
            })
        );
        assert_eq!(
            delta.head_branches,
            Some(BranchCounts {
                covered: 3,
                total: 4
            })
        );
        assert_eq!(delta.branch_diff.relevant, 4);
        assert_eq!(delta.branch_diff.covered, 3);
        assert_eq!(delta.branch_diff.partial_lines, vec![3]);
        assert!((comparison.branch_delta_pct().unwrap() - 25.0).abs() < 0.001);

        let totals = comparison.branch_diff_totals();
        assert_eq!(totals.relevant, 4);
        assert_eq!(totals.covered, 3);
    }

    #[test]
    fn unexecuted_changed_lines_are_not_branch_partial() {
        let head = snapshot(vec![file_with_branches(
            "a.py",
            &[(2, 0)],
            vec![branch(2, "0", None), branch(2, "1", None)],
        )]);
        let comparison = compare(None, &head, &changed("a.py", &[2]));

        let delta = &comparison.files[0];
        assert_eq!(delta.branch_diff.relevant, 2);
        assert_eq!(delta.branch_diff.covered, 0);
        assert!(delta.branch_diff.partial_lines.is_empty());
        assert_eq!(delta.diff.uncovered_lines, vec![2]);
    }

    #[test]
    fn branch_delta_requires_branch_data_on_both_sides() {
        let base = snapshot(vec![file("a.py", &[(1, 1)])]);
        let head = snapshot(vec![file_with_branches(
            "a.py",
            &[(1, 1)],
            vec![branch(1, "0", Some(1))],
        )]);
        let comparison = compare(Some(&base), &head, &ChangedLines::default());

        assert!(comparison.head_branch_totals().is_some());
        assert_eq!(comparison.base_branch_totals(), None);
        assert_eq!(comparison.branch_delta_pct(), None);
    }

    #[test]
    fn legacy_file_delta_json_defaults_branch_fields() {
        let json = r#"{
            "path": "a.py",
            "base": null,
            "head": {"covered": 1, "executable": 2},
            "diff": {"relevant": 0, "covered": 0, "uncovered_lines": []}
        }"#;
        let delta: FileDelta = serde_json::from_str(json).unwrap();
        assert_eq!(delta.base_branches, None);
        assert_eq!(delta.head_branches, None);
        assert!(delta.branch_diff.is_empty());

        let round_trip = serde_json::to_string(&delta).unwrap();
        assert!(!round_trip.contains("branch"));
    }

    #[test]
    fn comparison_document_round_trips() {
        let document = ComparisonDocument {
            schema_version: COMPARISON_SCHEMA_VERSION,
            head_sha: "abc123".into(),
            base_sha: Some("base123".into()),
            comparison: compare(None, &snapshot(vec![]), &ChangedLines::default()),
        };
        let json = serde_json::to_vec_pretty(&document).unwrap();
        let decoded: ComparisonDocument = serde_json::from_slice(&json).unwrap();
        assert_eq!(decoded, document);
    }

    #[test]
    fn analysis_document_preserves_wire_shape_and_document_compatibility() {
        let comparison = compare(None, &snapshot(vec![]), &ChangedLines::default());
        let document = ComparisonAnalysisDocument {
            schema_version: COMPARISON_SCHEMA_VERSION,
            head_sha: "abc123".into(),
            base_sha: Some("base123".into()),
            comparison: ComparisonAnalysis {
                comparison: comparison.clone(),
                scope_change: CoverageScopeChange {
                    appeared: vec!["appeared.py".into()],
                    disappeared: vec![],
                },
            },
        };

        let json = serde_json::to_vec(&document).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert!(value["comparison"]["files"].is_array());
        assert_eq!(
            value["comparison"]["scope_change"]["appeared"][0],
            "appeared.py"
        );

        let old_reader: ComparisonDocument = serde_json::from_slice(&json).unwrap();
        assert_eq!(old_reader.comparison, comparison);

        let old_reader = ComparisonDocument {
            schema_version: 1,
            ..old_reader
        };
        let old_json = serde_json::to_vec(&old_reader).unwrap();
        let new_reader: ComparisonAnalysisDocument = serde_json::from_slice(&old_json).unwrap();
        assert_eq!(new_reader.schema_version, 1);
        assert!(!new_reader.comparison.scope_changed());
        assert_eq!(new_reader.comparison.comparison, comparison);
    }

    #[test]
    fn comparison_schema_supports_legacy_and_current_versions_only() {
        assert!(!is_supported_comparison_schema_version(0));
        assert!(is_supported_comparison_schema_version(1));
        assert!(is_supported_comparison_schema_version(
            COMPARISON_SCHEMA_VERSION
        ));
        assert!(!is_supported_comparison_schema_version(
            COMPARISON_SCHEMA_VERSION + 1
        ));
    }

    #[test]
    fn detects_scope_expansion_and_contraction_from_snapshot_and_tree_sets() {
        let base = snapshot(vec![
            file("steady.py", &[(1, 1)]),
            file("disappeared.py", &[(1, 1)]),
        ]);
        let head = snapshot(vec![
            file("steady.py", &[(1, 1)]),
            file("appeared.py", &[(1, 0)]),
        ]);
        let base_tree = BTreeSet::from([
            "appeared.py".to_string(),
            "disappeared.py".to_string(),
            "steady.py".to_string(),
        ]);
        let head_tree = base_tree.clone();

        let comparison = compare_with_source_trees(
            Some(&base),
            &head,
            &ChangedLines::default(),
            Some(&base_tree),
            Some(&head_tree),
        );

        assert_eq!(comparison.scope_change.appeared, vec!["appeared.py"]);
        assert_eq!(comparison.scope_change.disappeared, vec!["disappeared.py"]);
        assert!(comparison.scope_changed());
        assert_eq!(comparison.delta_pct(), None);
        assert_eq!(
            comparison
                .files
                .iter()
                .find(|file| file.path == "steady.py")
                .unwrap()
                .delta_pct(),
            Some(0.0)
        );
    }

    #[test]
    fn scope_change_suppresses_branch_delta() {
        let base = snapshot(vec![
            file_with_branches("steady.py", &[(1, 1)], vec![branch(1, "0", Some(1))]),
            file("disappeared.py", &[(1, 1)]),
        ]);
        let head = snapshot(vec![
            file_with_branches("steady.py", &[(1, 1)], vec![branch(1, "0", Some(1))]),
            file("appeared.py", &[(1, 0)]),
        ]);
        let tree = BTreeSet::from([
            "appeared.py".to_string(),
            "disappeared.py".to_string(),
            "steady.py".to_string(),
        ]);

        let analysis = compare_with_source_trees(
            Some(&base),
            &head,
            &ChangedLines::default(),
            Some(&tree),
            Some(&tree),
        );

        assert!(analysis.scope_changed());
        assert_eq!(analysis.branch_delta_pct(), None);
        assert!(analysis.comparison.branch_delta_pct().is_some());
    }

    #[test]
    fn ignores_real_additions_deletions_and_renames_when_detecting_scope_changes() {
        let base = snapshot(vec![
            file("deleted.py", &[(1, 1)]),
            file("old_name.py", &[(1, 1)]),
        ]);
        let head = snapshot(vec![
            file("added.py", &[(1, 1)]),
            file("new_name.py", &[(1, 1)]),
        ]);
        let base_tree = BTreeSet::from(["deleted.py".to_string(), "old_name.py".to_string()]);
        let head_tree = BTreeSet::from(["added.py".to_string(), "new_name.py".to_string()]);

        let comparison = compare_with_source_trees(
            Some(&base),
            &head,
            &ChangedLines::default(),
            Some(&base_tree),
            Some(&head_tree),
        );

        assert!(!comparison.scope_changed());
        assert!(comparison.delta_pct().is_some());
    }

    #[test]
    fn ignores_scope_candidates_without_the_same_path_in_both_trees() {
        let base = snapshot(vec![file("base_only.py", &[(1, 1)])]);
        let head = snapshot(vec![file("head_only.py", &[(1, 1)])]);
        let base_tree = BTreeSet::from(["head_only.py".to_string()]);
        let head_tree = BTreeSet::from(["base_only.py".to_string()]);

        let comparison = compare_with_source_trees(
            Some(&base),
            &head,
            &ChangedLines::default(),
            Some(&base_tree),
            Some(&head_tree),
        );

        assert!(!comparison.scope_changed());
        assert!(comparison.scope_change.appeared.is_empty());
        assert!(comparison.scope_change.disappeared.is_empty());
    }

    #[test]
    fn old_comparison_json_defaults_to_unchanged_scope() {
        let comparison: ComparisonAnalysis =
            serde_json::from_str(r#"{"base_available":true,"files":[]}"#).unwrap();

        assert!(!comparison.scope_changed());
        assert!(comparison.scope_change.appeared.is_empty());
        assert!(comparison.scope_change.disappeared.is_empty());
    }

    #[test]
    fn affected_scope_entries_are_sorted_and_deduplicated() {
        let scope = CoverageScopeChange {
            appeared: vec!["z.py".into(), "a.py".into(), "a.py".into()],
            disappeared: vec!["d.py".into(), "c.py".into(), "c.py".into()],
        };

        let entries: Vec<_> = scope
            .affected_entries()
            .into_iter()
            .map(|entry| (entry.kind, entry.path))
            .collect();
        assert_eq!(
            entries,
            vec![
                (CoverageScopeChangeKind::Appeared, "a.py"),
                (CoverageScopeChangeKind::Appeared, "z.py"),
                (CoverageScopeChangeKind::Disappeared, "c.py"),
                (CoverageScopeChangeKind::Disappeared, "d.py"),
            ]
        );
    }
}
