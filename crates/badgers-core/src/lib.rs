//! Core data model for badgers coverage snapshots.
//!
//! This crate is intentionally free of I/O dependencies: it only defines the
//! snapshot schema and pure computations on top of it (totals, percentages,
//! normalization invariants).

pub mod compare;
pub mod diff;

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use serde::{Deserialize, Serialize};

/// Current snapshot schema version.
///
/// Policy: additive fields keep the version (with serde defaults); semantic
/// changes or removals bump it.
pub const SCHEMA_VERSION: u32 = 1;

/// Source language of a covered file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    Dart,
    Unknown,
}

impl Language {
    /// Infer the language from a repo-relative file path by extension.
    pub fn from_path(path: &str) -> Self {
        match std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
        {
            Some("rs") => Language::Rust,
            Some("py") => Language::Python,
            Some("dart") => Language::Dart,
            _ => Language::Unknown,
        }
    }
}

/// Execution count for a single line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineHit {
    /// 1-based line number.
    pub line: u32,
    /// Execution count (LCOV `DA` count).
    pub hits: u64,
}

/// Outcome count for a single branch of a decision point (LCOV `BRDA`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchHit {
    /// 1-based line number.
    pub line: u32,
    /// LCOV `BRDA` block identifier, kept verbatim.
    pub block: String,
    /// LCOV `BRDA` branch identifier or expression, kept verbatim.
    pub branch: String,
    /// Times the branch was taken; `None` when the decision point was never
    /// evaluated (LCOV `-`).
    pub taken: Option<u64>,
}

impl BranchHit {
    /// A branch counts as covered only when it was taken at least once.
    pub fn is_covered(&self) -> bool {
        self.taken.is_some_and(|taken| taken > 0)
    }

    /// LCOV tracefile merge rule for `BRDA` taken counts: `-` (never
    /// evaluated) merges as the other side; two counts sum (saturating).
    pub fn merge_taken(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        match (a, b) {
            (None, other) | (other, None) => other,
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
        }
    }
}

/// Execution count for a single function (LCOV `FN`/`FNDA`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionHit {
    pub name: String,
    /// 1-based line of the function start; 0 when the report omitted it.
    pub line: u32,
    pub hits: u64,
}

/// Outcome count for one MC/DC condition sense (LCOV `MCDC`).
///
/// `group`, `index`, and `expression` are kept verbatim; together with
/// `line` and `sense` (`t`/`f`) they identify one condition outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McdcHit {
    pub line: u32,
    pub group: String,
    pub sense: String,
    pub index: String,
    pub expression: String,
    /// `None` when the condition was never evaluated (LCOV `-`).
    pub taken: Option<u64>,
}

impl McdcHit {
    pub fn is_covered(&self) -> bool {
        self.taken.is_some_and(|taken| taken > 0)
    }

    /// Whether LCOV marked this condition outcome unreachable (`U<group_size>`).
    pub fn is_unreachable(&self) -> bool {
        Self::group_is_unreachable(&self.group)
    }

    /// Recognizes the official uppercase `U` prefix without interpreting other
    /// producer-specific group identifiers.
    pub fn group_is_unreachable(group: &str) -> bool {
        group
            .strip_prefix('U')
            .is_some_and(|size| !size.is_empty() && size.bytes().all(|b| b.is_ascii_digit()))
    }
}

/// Line coverage for one file.
///
/// `line_hits` is the canonical source of truth; executable/covered counts
/// are always derived from it, never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCoverage {
    /// Repo-root relative path with `/` separators.
    pub path: String,
    pub language: Language,
    /// Sorted by line, no duplicate lines (guaranteed by [`FileCoverage::new`]).
    pub line_hits: Vec<LineHit>,
    /// Sorted by (line, block, branch), no duplicates (guaranteed by
    /// [`FileCoverage::detailed`]). Additive field; absent in older snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<BranchHit>,
    /// Sorted by name, no duplicate names (guaranteed by
    /// [`FileCoverage::detailed`]). Additive field; absent in older snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionHit>,
    /// Sorted by (line, group, index, sense, expression), no duplicates.
    /// Additive field; absent in older snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcdc: Vec<McdcHit>,
    /// Distinct LCOV `TN` test names whose records covered this file, sorted.
    /// Additive field; absent in older snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_names: Vec<String>,
}

impl FileCoverage {
    /// Builds a line-only `FileCoverage`; see [`FileCoverage::detailed`].
    pub fn new(path: String, language: Language, line_hits: Vec<LineHit>) -> Self {
        Self::detailed(path, language, line_hits, Vec::new(), Vec::new())
    }

    /// Builds a `FileCoverage`, enforcing invariants: `line_hits` sorted by
    /// line with duplicates merged by summing hits (saturating); `branches`
    /// sorted by (line, block, branch) with duplicate taken-counts merged
    /// (LCOV rule: `-` merges as the other side, counts sum); `functions`
    /// sorted by name with hits summed and the smallest start line kept.
    pub fn detailed(
        path: String,
        language: Language,
        line_hits: Vec<LineHit>,
        branches: Vec<BranchHit>,
        functions: Vec<FunctionHit>,
    ) -> Self {
        Self {
            path,
            language,
            line_hits,
            branches,
            functions,
            mcdc: Vec::new(),
            test_names: Vec::new(),
        }
        .normalized()
    }

    /// Replaces MC/DC data and re-normalizes; see [`FileCoverage::detailed`].
    pub fn with_mcdc(mut self, mcdc: Vec<McdcHit>) -> Self {
        self.mcdc = mcdc;
        self.normalized()
    }

    /// Replaces test-name attribution and re-normalizes.
    pub fn with_test_names(mut self, test_names: Vec<String>) -> Self {
        self.test_names = test_names;
        self.normalized()
    }

    fn normalized(self) -> Self {
        let mut merged: BTreeMap<u32, u64> = BTreeMap::new();
        for lh in self.line_hits {
            let slot = merged.entry(lh.line).or_insert(0);
            *slot = slot.saturating_add(lh.hits);
        }

        let mut merged_branches: BTreeMap<(u32, String, String), Option<u64>> = BTreeMap::new();
        for branch in self.branches {
            let slot = merged_branches
                .entry((branch.line, branch.block, branch.branch))
                .or_insert(None);
            *slot = BranchHit::merge_taken(*slot, branch.taken);
        }

        let mut merged_functions: BTreeMap<String, (u32, u64)> = BTreeMap::new();
        for function in self.functions {
            let slot = merged_functions
                .entry(function.name)
                .or_insert((function.line, 0));
            if function.line != 0 && (slot.0 == 0 || function.line < slot.0) {
                slot.0 = function.line;
            }
            slot.1 = slot.1.saturating_add(function.hits);
        }

        let mut merged_mcdc: BTreeMap<(u32, String, String, String, String), Option<u64>> =
            BTreeMap::new();
        for hit in self.mcdc {
            let slot = merged_mcdc
                .entry((hit.line, hit.group, hit.index, hit.sense, hit.expression))
                .or_insert(None);
            *slot = BranchHit::merge_taken(*slot, hit.taken);
        }

        let mut test_names = self.test_names;
        test_names.sort_unstable();
        test_names.dedup();

        Self {
            path: self.path,
            language: self.language,
            line_hits: merged
                .into_iter()
                .map(|(line, hits)| LineHit { line, hits })
                .collect(),
            branches: merged_branches
                .into_iter()
                .map(|((line, block, branch), taken)| BranchHit {
                    line,
                    block,
                    branch,
                    taken,
                })
                .collect(),
            functions: merged_functions
                .into_iter()
                .map(|(name, (line, hits))| FunctionHit { name, line, hits })
                .collect(),
            mcdc: merged_mcdc
                .into_iter()
                .map(|((line, group, index, sense, expression), taken)| McdcHit {
                    line,
                    group,
                    sense,
                    index,
                    expression,
                    taken,
                })
                .collect(),
            test_names,
        }
    }

    /// Number of executable lines (lines with any `DA` record).
    pub fn executable_lines(&self) -> u32 {
        self.line_hits.len() as u32
    }

    /// Number of executable lines with at least one hit.
    pub fn covered_lines(&self) -> u32 {
        self.line_hits.iter().filter(|lh| lh.hits > 0).count() as u32
    }

    /// Coverage percentage, `None` when there are no executable lines.
    pub fn coverage_pct(&self) -> Option<f64> {
        coverage_pct(
            u64::from(self.covered_lines()),
            u64::from(self.executable_lines()),
        )
    }

    /// Number of branches (all `BRDA` outcomes, including never-evaluated).
    pub fn total_branches(&self) -> u32 {
        self.branches.len() as u32
    }

    /// Number of branches taken at least once.
    pub fn covered_branches(&self) -> u32 {
        self.branches.iter().filter(|b| b.is_covered()).count() as u32
    }

    /// Branch coverage percentage, `None` when there are no branches.
    pub fn branch_pct(&self) -> Option<f64> {
        coverage_pct(
            u64::from(self.covered_branches()),
            u64::from(self.total_branches()),
        )
    }

    /// Number of instrumented functions.
    pub fn total_functions(&self) -> u32 {
        self.functions.len() as u32
    }

    /// Number of functions executed at least once.
    pub fn covered_functions(&self) -> u32 {
        self.functions.iter().filter(|f| f.hits > 0).count() as u32
    }

    /// Function coverage percentage, `None` when there are no functions.
    pub fn function_pct(&self) -> Option<f64> {
        coverage_pct(
            u64::from(self.covered_functions()),
            u64::from(self.total_functions()),
        )
    }

    /// Number of reachable MC/DC condition outcomes.
    pub fn total_mcdc(&self) -> u32 {
        self.mcdc.iter().filter(|m| !m.is_unreachable()).count() as u32
    }

    /// Number of reachable MC/DC condition outcomes taken at least once.
    pub fn covered_mcdc(&self) -> u32 {
        self.mcdc
            .iter()
            .filter(|m| !m.is_unreachable() && m.is_covered())
            .count() as u32
    }

    /// MC/DC coverage percentage, `None` without MC/DC data.
    pub fn mcdc_pct(&self) -> Option<f64> {
        coverage_pct(u64::from(self.covered_mcdc()), u64::from(self.total_mcdc()))
    }
}

/// `covered / executable * 100`, or `None` when `executable == 0`.
///
/// Percentages are never persisted; they are always recomputed from the
/// integer pair to avoid accumulated floating point drift.
pub fn coverage_pct(covered: u64, executable: u64) -> Option<f64> {
    (executable > 0).then(|| covered as f64 / executable as f64 * 100.0)
}

/// Versions of the tools that produced a snapshot, for reproducibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolVersions {
    pub badgers: String,
    pub cargo_llvm_cov: Option<String>,
    pub coverage_py: Option<String>,
}

/// Full coverage measurement for one commit.
///
/// This is the canonical artifact stored as `coverage.json.zst`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageSnapshot {
    pub schema_version: u32,
    /// e.g. "jubilee-works/timetree-planner-server".
    pub repo: String,
    /// Full 40-char commit SHA.
    pub commit_sha: String,
    pub branch: Option<String>,
    pub pr_number: Option<u64>,
    /// RFC 3339 UTC timestamp.
    pub generated_at: String,
    pub tool_versions: ToolVersions,
    /// Sorted by path, no duplicate paths (guaranteed by [`CoverageSnapshot::new`]).
    pub files: Vec<FileCoverage>,
}

impl CoverageSnapshot {
    /// Builds a snapshot, enforcing invariants: files sorted by path with
    /// duplicate paths merged (line hits summed), each file normalized.
    pub fn new(
        repo: String,
        commit_sha: String,
        branch: Option<String>,
        pr_number: Option<u64>,
        generated_at: String,
        tool_versions: ToolVersions,
        files: Vec<FileCoverage>,
    ) -> Self {
        let mut by_path: BTreeMap<String, FileCoverage> = BTreeMap::new();
        for fc in files {
            let fc = fc.normalized();
            match by_path.entry(fc.path.clone()) {
                Entry::Vacant(v) => {
                    v.insert(fc);
                }
                Entry::Occupied(mut o) => {
                    let existing = o.get_mut();
                    existing.line_hits.extend(fc.line_hits);
                    existing.branches.extend(fc.branches);
                    existing.functions.extend(fc.functions);
                    existing.mcdc.extend(fc.mcdc);
                    existing.test_names.extend(fc.test_names);
                    let combined = std::mem::replace(
                        existing,
                        FileCoverage::new(String::new(), Language::Unknown, Vec::new()),
                    );
                    *existing = combined.normalized();
                }
            }
        }
        Self {
            schema_version: SCHEMA_VERSION,
            repo,
            commit_sha,
            branch,
            pr_number,
            generated_at,
            tool_versions,
            files: by_path.into_values().collect(),
        }
    }

    /// Total executable lines across all files.
    pub fn executable_lines(&self) -> u64 {
        self.files
            .iter()
            .map(|f| u64::from(f.executable_lines()))
            .sum()
    }

    /// Total covered lines across all files.
    pub fn covered_lines(&self) -> u64 {
        self.files
            .iter()
            .map(|f| u64::from(f.covered_lines()))
            .sum()
    }

    /// Total coverage percentage, `None` when there are no executable lines.
    pub fn coverage_pct(&self) -> Option<f64> {
        coverage_pct(self.covered_lines(), self.executable_lines())
    }

    /// Total branches across all files.
    pub fn total_branches(&self) -> u64 {
        self.files
            .iter()
            .map(|f| u64::from(f.total_branches()))
            .sum()
    }

    /// Total covered branches across all files.
    pub fn covered_branches(&self) -> u64 {
        self.files
            .iter()
            .map(|f| u64::from(f.covered_branches()))
            .sum()
    }

    /// Total branch coverage percentage, `None` when there are no branches.
    pub fn branch_pct(&self) -> Option<f64> {
        coverage_pct(self.covered_branches(), self.total_branches())
    }

    /// Total instrumented functions across all files.
    pub fn total_functions(&self) -> u64 {
        self.files
            .iter()
            .map(|f| u64::from(f.total_functions()))
            .sum()
    }

    /// Total covered functions across all files.
    pub fn covered_functions(&self) -> u64 {
        self.files
            .iter()
            .map(|f| u64::from(f.covered_functions()))
            .sum()
    }

    /// Total function coverage percentage, `None` when there are no functions.
    pub fn function_pct(&self) -> Option<f64> {
        coverage_pct(self.covered_functions(), self.total_functions())
    }

    /// Total MC/DC condition outcomes across all files.
    pub fn total_mcdc(&self) -> u64 {
        self.files.iter().map(|f| u64::from(f.total_mcdc())).sum()
    }

    /// Total covered MC/DC condition outcomes across all files.
    pub fn covered_mcdc(&self) -> u64 {
        self.files.iter().map(|f| u64::from(f.covered_mcdc())).sum()
    }

    /// Total MC/DC coverage percentage, `None` without MC/DC data.
    pub fn mcdc_pct(&self) -> Option<f64> {
        coverage_pct(self.covered_mcdc(), self.total_mcdc())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with(files: Vec<FileCoverage>) -> CoverageSnapshot {
        CoverageSnapshot::new(
            "owner/repo".to_string(),
            "a".repeat(40),
            Some("main".to_string()),
            None,
            "2026-07-04T00:00:00Z".to_string(),
            ToolVersions {
                badgers: "0.1.0".to_string(),
                cargo_llvm_cov: None,
                coverage_py: Some("7.6.1".to_string()),
            },
            files,
        )
    }

    #[test]
    fn file_coverage_merges_duplicate_lines_and_sorts() {
        let fc = FileCoverage::new(
            "a.py".to_string(),
            Language::Python,
            vec![
                LineHit { line: 5, hits: 2 },
                LineHit { line: 1, hits: 0 },
                LineHit { line: 5, hits: 3 },
            ],
        );
        assert_eq!(
            fc.line_hits,
            vec![LineHit { line: 1, hits: 0 }, LineHit { line: 5, hits: 5 }]
        );
        assert_eq!(fc.executable_lines(), 2);
        assert_eq!(fc.covered_lines(), 1);
    }

    #[test]
    fn snapshot_sorts_and_merges_duplicate_paths() {
        let snap = snapshot_with(vec![
            FileCoverage::new(
                "b.py".to_string(),
                Language::Python,
                vec![LineHit { line: 1, hits: 1 }],
            ),
            FileCoverage::new(
                "a.py".to_string(),
                Language::Python,
                vec![LineHit { line: 1, hits: 0 }],
            ),
            FileCoverage::new(
                "a.py".to_string(),
                Language::Python,
                vec![LineHit { line: 1, hits: 4 }, LineHit { line: 2, hits: 0 }],
            ),
        ]);
        let paths: Vec<&str> = snap.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.py", "b.py"]);
        assert_eq!(
            snap.files[0].line_hits,
            vec![LineHit { line: 1, hits: 4 }, LineHit { line: 2, hits: 0 }]
        );
        assert_eq!(snap.executable_lines(), 3);
        assert_eq!(snap.covered_lines(), 2);
    }

    #[test]
    fn coverage_pct_is_none_without_executable_lines() {
        assert_eq!(coverage_pct(0, 0), None);
        let snap = snapshot_with(vec![]);
        assert_eq!(snap.coverage_pct(), None);
        let pct = coverage_pct(2, 3).unwrap();
        assert!((pct - 66.666_666).abs() < 0.001);
    }

    #[test]
    fn language_from_path() {
        assert_eq!(Language::from_path("src/lib.rs"), Language::Rust);
        assert_eq!(Language::from_path("pkg/mod.py"), Language::Python);
        assert_eq!(Language::from_path("lib/main.dart"), Language::Dart);
        assert_eq!(Language::from_path("README.md"), Language::Unknown);
        assert_eq!(Language::from_path("Makefile"), Language::Unknown);
    }

    #[test]
    fn serde_round_trip() {
        let snap = snapshot_with(vec![FileCoverage::new(
            "pkg/app.py".to_string(),
            Language::Python,
            vec![LineHit { line: 1, hits: 1 }, LineHit { line: 2, hits: 0 }],
        )]);
        let json = serde_json::to_string_pretty(&snap).unwrap();
        let back: CoverageSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
        assert!(json.contains("\"language\": \"python\""));
        assert_eq!(back.schema_version, SCHEMA_VERSION);
    }

    fn branch(line: u32, branch: &str, taken: Option<u64>) -> BranchHit {
        BranchHit {
            line,
            block: "0".to_string(),
            branch: branch.to_string(),
            taken,
        }
    }

    #[test]
    fn merges_branch_taken_counts_with_lcov_dash_rule() {
        let fc = FileCoverage::detailed(
            "a.py".to_string(),
            Language::Python,
            vec![LineHit { line: 1, hits: 1 }],
            vec![
                branch(2, "0", None),
                branch(2, "0", Some(2)),
                branch(2, "1", None),
                branch(2, "1", None),
                branch(2, "2", Some(0)),
            ],
            Vec::new(),
        );
        assert_eq!(
            fc.branches,
            vec![
                branch(2, "0", Some(2)),
                branch(2, "1", None),
                branch(2, "2", Some(0)),
            ]
        );
        assert_eq!(fc.total_branches(), 3);
        assert_eq!(fc.covered_branches(), 1);
        let pct = fc.branch_pct().unwrap();
        assert!((pct - 33.333_333).abs() < 0.001);
    }

    #[test]
    fn merges_functions_by_name_and_keeps_smallest_known_line() {
        let fc = FileCoverage::detailed(
            "a.py".to_string(),
            Language::Python,
            Vec::new(),
            Vec::new(),
            vec![
                FunctionHit {
                    name: "main".to_string(),
                    line: 0,
                    hits: 1,
                },
                FunctionHit {
                    name: "main".to_string(),
                    line: 4,
                    hits: 2,
                },
                FunctionHit {
                    name: "helper".to_string(),
                    line: 9,
                    hits: 0,
                },
            ],
        );
        assert_eq!(
            fc.functions,
            vec![
                FunctionHit {
                    name: "helper".to_string(),
                    line: 9,
                    hits: 0,
                },
                FunctionHit {
                    name: "main".to_string(),
                    line: 4,
                    hits: 3,
                },
            ]
        );
        assert_eq!(fc.total_functions(), 2);
        assert_eq!(fc.covered_functions(), 1);
    }

    #[test]
    fn snapshot_merges_branches_and_functions_across_duplicate_paths() {
        let snap = snapshot_with(vec![
            FileCoverage::detailed(
                "a.py".to_string(),
                Language::Python,
                vec![LineHit { line: 1, hits: 1 }],
                vec![branch(1, "0", None)],
                vec![FunctionHit {
                    name: "main".to_string(),
                    line: 1,
                    hits: 1,
                }],
            ),
            FileCoverage::detailed(
                "a.py".to_string(),
                Language::Python,
                vec![LineHit { line: 1, hits: 1 }],
                vec![branch(1, "0", Some(3)), branch(1, "1", Some(0))],
                vec![FunctionHit {
                    name: "main".to_string(),
                    line: 1,
                    hits: 2,
                }],
            ),
        ]);
        assert_eq!(snap.files.len(), 1);
        assert_eq!(
            snap.files[0].branches,
            vec![branch(1, "0", Some(3)), branch(1, "1", Some(0))]
        );
        assert_eq!(snap.total_branches(), 2);
        assert_eq!(snap.covered_branches(), 1);
        assert_eq!(snap.files[0].functions[0].hits, 3);
        assert_eq!(snap.total_functions(), 1);
        assert_eq!(snap.covered_functions(), 1);
    }

    #[test]
    fn mcdc_totals_exclude_only_official_unreachable_groups() {
        let file =
            FileCoverage::new("a.c".to_string(), Language::Unknown, Vec::new()).with_mcdc(vec![
                McdcHit {
                    line: 1,
                    group: "2".to_string(),
                    sense: "t".to_string(),
                    index: "0".to_string(),
                    expression: "0".to_string(),
                    taken: Some(1),
                },
                McdcHit {
                    line: 1,
                    group: "U2".to_string(),
                    sense: "f".to_string(),
                    index: "0".to_string(),
                    expression: "0".to_string(),
                    taken: Some(4),
                },
                McdcHit {
                    line: 2,
                    group: "u2".to_string(),
                    sense: "t".to_string(),
                    index: "0".to_string(),
                    expression: "0".to_string(),
                    taken: Some(0),
                },
                McdcHit {
                    line: 3,
                    group: "UserGroup".to_string(),
                    sense: "t".to_string(),
                    index: "0".to_string(),
                    expression: "0".to_string(),
                    taken: Some(1),
                },
            ]);

        assert_eq!(file.mcdc.len(), 4);
        assert_eq!(file.total_mcdc(), 3);
        assert_eq!(file.covered_mcdc(), 2);
        let snapshot = snapshot_with(vec![file]);
        assert_eq!(snapshot.total_mcdc(), 3);
        assert_eq!(snapshot.covered_mcdc(), 2);
    }

    #[test]
    fn line_only_snapshots_stay_schema_compatible() {
        let line_only = snapshot_with(vec![FileCoverage::new(
            "a.py".to_string(),
            Language::Python,
            vec![LineHit { line: 1, hits: 1 }],
        )]);
        let json = serde_json::to_string_pretty(&line_only).unwrap();
        assert!(!json.contains("\"branches\""));
        assert!(!json.contains("\"functions\""));
        assert!(!json.contains("\"mcdc\""));
        assert!(!json.contains("\"test_names\""));
        assert_eq!(line_only.schema_version, SCHEMA_VERSION);

        let back: CoverageSnapshot = serde_json::from_str(&json).unwrap();
        assert!(back.files[0].branches.is_empty());
        assert!(back.files[0].functions.is_empty());
        assert_eq!(back.branch_pct(), None);
        assert_eq!(back.function_pct(), None);
    }
}
