//! llvm-cov JSON export parsing for MC/DC enrichment.
//!
//! `llvm-cov export -format=lcov` never emits MC/DC records, so Badgers
//! accepts the JSON export (`-format=text`) as an optional sidecar and maps
//! each MC/DC condition onto the [`McdcHit`] model. LLVM reports condition
//! (independence-pair) coverage rather than GCC's per-sense outcomes, so
//! synthesized hits use `sense = "c"` and an `llvm:`-prefixed group; the two
//! encodings are never mixed in one snapshot.
//!
//! Only `data[*].files[*].mcdc_records` are read. Function-level records
//! duplicate the file-level ones and are ignored. Supported schema majors are
//! 2 (LLVM 18-20) and 3 (LLVM 21+); newer majors skip the sidecar with a
//! warning instead of guessing field positions.

use std::collections::BTreeMap;

use badge_rs_core::{FileCoverage, McdcHit};
use serde_json::Value;

use crate::{ParseOptions, normalize_sf_path};

/// LLVM's `CoverageMapping::RegionKind` value for MC/DC decision regions.
const MCDC_DECISION_REGION_KIND: u64 = 5;

#[derive(Debug, thiserror::Error)]
pub enum LlvmJsonError {
    #[error("llvm-cov JSON error: {0}")]
    Malformed(String),
    #[error(
        "the LCOV input already contains MCDC records; refusing to mix GCC \
         sense outcomes with LLVM condition coverage from the sidecar"
    )]
    MixedMcdcSources,
}

fn malformed(message: impl Into<String>) -> LlvmJsonError {
    LlvmJsonError::Malformed(message.into())
}

/// Enriches LCOV-derived files with MC/DC conditions from an llvm-cov JSON
/// export. Returns non-fatal warnings; files whose records disagree with the
/// document's own summaries are skipped rather than published with a wrong
/// denominator (llvm-cov excludes folded conditions from `summary.mcdc` but
/// does not identify them in `mcdc_records`).
pub fn enrich_mcdc_from_llvm_json(
    input: &str,
    opts: &ParseOptions<'_>,
    files: &mut [FileCoverage],
) -> Result<Vec<String>, LlvmJsonError> {
    if files.iter().any(|file| !file.mcdc.is_empty()) {
        return Err(LlvmJsonError::MixedMcdcSources);
    }

    let doc: Value =
        serde_json::from_str(input).map_err(|err| malformed(format!("invalid JSON: {err}")))?;
    let doc_type = doc.get("type").and_then(Value::as_str).unwrap_or_default();
    if doc_type != "llvm.coverage.json.export" {
        return Err(malformed(format!(
            "unexpected document type '{doc_type}' (want 'llvm.coverage.json.export')"
        )));
    }

    let mut warnings = Vec::new();
    let version = doc
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(layout) = RecordLayout::from_version(version) else {
        warnings.push(format!(
            "llvm-cov JSON schema version '{version}' is not supported; MC/DC sidecar skipped"
        ));
        return Ok(warnings);
    };

    let entries = doc
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("missing 'data' array"))?;

    // Records for the same path across repeated `data`/`files` entries get
    // document-wide ordinals so their identities never collide.
    let mut ordinals: BTreeMap<String, u64> = BTreeMap::new();
    for entry in entries {
        let file_objects = entry
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("missing 'files' array in data entry"))?;
        for object in file_objects {
            enrich_file(object, layout, opts, files, &mut ordinals, &mut warnings);
        }
    }
    Ok(warnings)
}

/// Positions of the fields Badgers reads from one `mcdc_records` element.
#[derive(Debug, Clone, Copy)]
struct RecordLayout {
    /// Total array lengths this schema major emits.
    lengths: &'static [usize],
    kind: usize,
    conditions: usize,
    /// `(FileID, ExpandedFileID)` positions; `FileID` is absent in major 2.
    file_id: Option<usize>,
    expanded_file_id: usize,
}

impl RecordLayout {
    fn from_version(version: &str) -> Option<Self> {
        // Major 2 (LLVM 18-20):
        //   [LineStart, ColumnStart, LineEnd, ColumnEnd, ExpandedFileID,
        //    Kind, Conditions[]]
        // Major 3 (LLVM 21+):
        //   [LineStart, ColumnStart, LineEnd, ColumnEnd, TrueDecisions,
        //    FalseDecisions, FileID, ExpandedFileID, Kind, Conditions[]]
        //   plus trailing TestVectors[] from LLVM 22 (schema 3.1.0).
        match version.split('.').next() {
            Some("2") => Some(Self {
                lengths: &[7],
                kind: 5,
                conditions: 6,
                file_id: None,
                expanded_file_id: 4,
            }),
            Some("3") => Some(Self {
                lengths: &[10, 11],
                kind: 8,
                conditions: 9,
                file_id: Some(6),
                expanded_file_id: 7,
            }),
            _ => None,
        }
    }
}

fn enrich_file(
    object: &Value,
    layout: RecordLayout,
    opts: &ParseOptions<'_>,
    files: &mut [FileCoverage],
    ordinals: &mut BTreeMap<String, u64>,
    warnings: &mut Vec<String>,
) {
    let Some(raw_path) = object.get("filename").and_then(Value::as_str) else {
        warnings.push("llvm-cov JSON file entry without 'filename' skipped".to_string());
        return;
    };
    let Some(path) = normalize_sf_path(raw_path, opts.repo_root) else {
        warnings.push(format!(
            "{raw_path}: path resolves outside repo root, MC/DC sidecar entry skipped"
        ));
        return;
    };
    let Some(file) = files.iter_mut().find(|file| file.path == path) else {
        warnings.push(format!(
            "{path}: no matching file in the LCOV input, MC/DC sidecar entry skipped"
        ));
        return;
    };

    let Some((declared_lines, declared_mcdc)) = read_summary(object) else {
        warnings.push(format!(
            "{path}: llvm-cov JSON summary missing or malformed, MC/DC skipped"
        ));
        return;
    };
    if declared_lines
        != (
            u64::from(file.executable_lines()),
            u64::from(file.covered_lines()),
        )
    {
        warnings.push(format!(
            "{path}: llvm-cov JSON line summary {}/{} disagrees with LCOV {}/{}; \
             stale sidecar? MC/DC skipped",
            declared_lines.1,
            declared_lines.0,
            file.covered_lines(),
            file.executable_lines(),
        ));
        return;
    }

    let records = object
        .get("mcdc_records")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut hits = Vec::new();
    let mut derived = (0u64, 0u64);
    for record in &records {
        let ordinal = ordinals.entry(path.clone()).or_insert(0);
        *ordinal += 1;
        match parse_record(record, layout, *ordinal) {
            Ok(mut record_hits) => {
                derived.0 += record_hits.len() as u64;
                derived.1 += record_hits.iter().filter(|hit| hit.is_covered()).count() as u64;
                hits.append(&mut record_hits);
            }
            Err(reason) => {
                warnings.push(format!("{path}: {reason}; record skipped"));
            }
        }
    }
    if derived != declared_mcdc {
        warnings.push(format!(
            "{path}: {}/{} MC/DC conditions from records disagree with llvm-cov \
             summary {}/{} (folded conditions are not identified in the JSON); \
             MC/DC skipped",
            derived.1, derived.0, declared_mcdc.1, declared_mcdc.0,
        ));
        return;
    }
    file.mcdc.extend(hits);
}

/// Reads `summary.lines` and `summary.mcdc` as `(count, covered)` pairs.
fn read_summary(object: &Value) -> Option<((u64, u64), (u64, u64))> {
    let summary = object.get("summary")?;
    let pair = |section: &str| {
        let section = summary.get(section)?;
        Some((
            section.get("count")?.as_u64()?,
            section.get("covered")?.as_u64()?,
        ))
    };
    Some((pair("lines")?, pair("mcdc")?))
}

fn parse_record(
    record: &Value,
    layout: RecordLayout,
    ordinal: u64,
) -> Result<Vec<McdcHit>, String> {
    let fields = record
        .as_array()
        .ok_or_else(|| "malformed MCDC record (not an array)".to_string())?;
    if !layout.lengths.contains(&fields.len()) {
        return Err(format!(
            "malformed MCDC record ({} fields, expected {:?})",
            fields.len(),
            layout.lengths
        ));
    }
    let uint = |index: usize, name: &str| {
        fields
            .get(index)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("malformed MCDC record ({name} is not an unsigned integer)"))
    };
    let kind = uint(layout.kind, "kind")?;
    if kind != MCDC_DECISION_REGION_KIND {
        return Err(format!("unexpected MCDC record kind {kind}"));
    }
    let line: u32 = uint(0, "line")?
        .try_into()
        .map_err(|_| "malformed MCDC record (line out of range)".to_string())?;
    if line == 0 {
        return Err("malformed MCDC record (line is zero)".to_string());
    }
    let column_start = uint(1, "column start")?;
    let line_end = uint(2, "line end")?;
    let column_end = uint(3, "column end")?;
    let expanded_file_id = uint(layout.expanded_file_id, "expanded file id")?;
    let file_id = layout
        .file_id
        .map(|index| uint(index, "file id"))
        .transpose()?;

    let conditions = fields
        .get(layout.conditions)
        .and_then(Value::as_array)
        .ok_or_else(|| "malformed MCDC record (conditions is not an array)".to_string())?;
    let group = match file_id {
        Some(file_id) => format!(
            "llvm:{line}:{column_start}-{line_end}:{column_end}:{file_id}.{expanded_file_id}:{ordinal}"
        ),
        None => {
            format!(
                "llvm:{line}:{column_start}-{line_end}:{column_end}:{expanded_file_id}:{ordinal}"
            )
        }
    };
    conditions
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let covered = value.as_bool().ok_or_else(|| {
                "malformed MCDC record (condition entry is not a boolean)".to_string()
            })?;
            Ok(McdcHit {
                line,
                group: group.clone(),
                sense: "c".to_string(),
                index: index.to_string(),
                expression: String::new(),
                taken: Some(u64::from(covered)),
            })
        })
        .collect()
}
