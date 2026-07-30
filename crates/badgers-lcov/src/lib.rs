//! LCOV parsing into badgers coverage models.
//!
//! Line records (`SF`, `DA`, `LF`, `LH`, `end_of_record`) are strict: malformed
//! input fails the parse. Function (`FN`, `FNDA`, `FNF`, `FNH`) and branch
//! (`BRDA`, `BRF`, `BRH`) records are best-effort: malformed entries are
//! skipped with a warning so reports from older tools keep parsing. `TN`,
//! `VER`, MC/DC records, and `DA` checksums are ignored.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use badge_rs_core::{BranchHit, FileCoverage, FunctionHit, Language, LineHit};

#[derive(Debug)]
pub struct ParseOptions<'a> {
    /// Absolute repository root used to relativize absolute `SF:` paths.
    pub repo_root: &'a Path,
}

#[derive(Debug)]
pub struct ParseOutcome {
    /// Sorted by path; same-path records are merged with hits summed.
    pub files: Vec<FileCoverage>,
    /// Non-fatal issues: count mismatches, malformed optional records,
    /// dropped out-of-root paths, etc.
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LcovError {
    #[error("lcov parse error at line {line}: {message}")]
    Malformed { line: usize, message: String },
}

fn malformed(line: usize, message: impl Into<String>) -> LcovError {
    LcovError::Malformed {
        line,
        message: message.into(),
    }
}

struct Block {
    raw_path: String,
    hits: BTreeMap<u32, u64>,
    branches: BTreeMap<(u32, String, String), Option<u64>>,
    functions: BTreeMap<String, (u32, u64)>,
    lf: Option<u64>,
    lh: Option<u64>,
    fnf: Option<u64>,
    fnh: Option<u64>,
    brf: Option<u64>,
    brh: Option<u64>,
}

impl Block {
    fn new(raw_path: String) -> Self {
        Self {
            raw_path,
            hits: BTreeMap::new(),
            branches: BTreeMap::new(),
            functions: BTreeMap::new(),
            lf: None,
            lh: None,
            fnf: None,
            fnh: None,
            brf: None,
            brh: None,
        }
    }
}

#[derive(Default)]
struct FileAcc {
    hits: BTreeMap<u32, u64>,
    branches: Vec<BranchHit>,
    functions: Vec<FunctionHit>,
}

pub fn parse_lcov(input: &str, opts: &ParseOptions<'_>) -> Result<ParseOutcome, LcovError> {
    let mut merged: BTreeMap<String, FileAcc> = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut current: Option<Block> = None;
    let mut last_lineno = 0;

    for (idx, raw) in input.lines().enumerate() {
        let lineno = idx + 1;
        last_lineno = lineno;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line == "end_of_record" {
            let block = current
                .take()
                .ok_or_else(|| malformed(lineno, "end_of_record without preceding SF"))?;
            finish_block(block, opts, &mut merged, &mut warnings);
            continue;
        }
        let Some((tag, rest)) = line.split_once(':') else {
            warnings.push(format!("line {lineno}: unrecognized line skipped: {line}"));
            continue;
        };
        match tag {
            "SF" => {
                if current.is_some() {
                    return Err(malformed(
                        lineno,
                        "SF while previous record is still open (missing end_of_record)",
                    ));
                }
                let path = rest.trim();
                if path.is_empty() {
                    return Err(malformed(lineno, "SF with empty path"));
                }
                current = Some(Block::new(path.to_string()));
            }
            "DA" => {
                let block = current
                    .as_mut()
                    .ok_or_else(|| malformed(lineno, "DA before SF"))?;
                // Format: DA:<line>,<hits>[,<checksum>] - checksum is ignored.
                let mut fields = rest.split(',');
                let line_field = fields
                    .next()
                    .ok_or_else(|| malformed(lineno, "DA missing line number"))?;
                let line_no: u32 = line_field.trim().parse().map_err(|_| {
                    malformed(lineno, format!("DA has invalid line number '{line_field}'"))
                })?;
                let hits_field = fields
                    .next()
                    .ok_or_else(|| malformed(lineno, "DA missing hit count"))?;
                let hits: u64 = hits_field.trim().parse().map_err(|_| {
                    malformed(lineno, format!("DA has invalid hit count '{hits_field}'"))
                })?;
                let slot = block.hits.entry(line_no).or_insert(0);
                *slot = slot.saturating_add(hits);
            }
            "LF" => {
                let block = current
                    .as_mut()
                    .ok_or_else(|| malformed(lineno, "LF before SF"))?;
                block.lf = Some(parse_count(lineno, "LF", rest)?);
            }
            "LH" => {
                let block = current
                    .as_mut()
                    .ok_or_else(|| malformed(lineno, "LH before SF"))?;
                block.lh = Some(parse_count(lineno, "LH", rest)?);
            }
            "FN" => {
                let Some(block) = current.as_mut() else {
                    warnings.push(format!("line {lineno}: FN before SF skipped"));
                    continue;
                };
                match parse_fn(rest) {
                    Some((start_line, name)) => {
                        let slot = block.functions.entry(name.to_string()).or_insert((0, 0));
                        if start_line != 0 && (slot.0 == 0 || start_line < slot.0) {
                            slot.0 = start_line;
                        }
                    }
                    None => warnings.push(format!("line {lineno}: malformed FN skipped: {line}")),
                }
            }
            "FNDA" => {
                let Some(block) = current.as_mut() else {
                    warnings.push(format!("line {lineno}: FNDA before SF skipped"));
                    continue;
                };
                match parse_fnda(rest) {
                    Some((hits, name)) => {
                        let slot = block.functions.entry(name.to_string()).or_insert((0, 0));
                        slot.1 = slot.1.saturating_add(hits);
                    }
                    None => warnings.push(format!("line {lineno}: malformed FNDA skipped: {line}")),
                }
            }
            "BRDA" => {
                let Some(block) = current.as_mut() else {
                    warnings.push(format!("line {lineno}: BRDA before SF skipped"));
                    continue;
                };
                match parse_brda(rest) {
                    Some((line_no, block_id, branch_id, taken)) => {
                        let slot = block
                            .branches
                            .entry((line_no, block_id, branch_id))
                            .or_insert(None);
                        *slot = BranchHit::merge_taken(*slot, taken);
                    }
                    None => warnings.push(format!("line {lineno}: malformed BRDA skipped: {line}")),
                }
            }
            "FNF" | "FNH" | "BRF" | "BRH" => {
                let Some(block) = current.as_mut() else {
                    warnings.push(format!("line {lineno}: {tag} before SF skipped"));
                    continue;
                };
                match rest.trim().parse::<u64>() {
                    Ok(count) => {
                        let slot = match tag {
                            "FNF" => &mut block.fnf,
                            "FNH" => &mut block.fnh,
                            "BRF" => &mut block.brf,
                            _ => &mut block.brh,
                        };
                        *slot = Some(count);
                    }
                    Err(_) => {
                        warnings.push(format!("line {lineno}: {tag} has invalid count '{rest}'"))
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(block) = current {
        return Err(malformed(
            last_lineno,
            format!(
                "unterminated record for '{}' (missing end_of_record)",
                block.raw_path
            ),
        ));
    }

    let files = merged
        .into_iter()
        .map(|(path, acc)| {
            let language = Language::from_path(&path);
            FileCoverage::detailed(
                path,
                language,
                acc.hits
                    .into_iter()
                    .map(|(line, hits)| LineHit { line, hits })
                    .collect(),
                acc.branches,
                acc.functions,
            )
        })
        .collect();

    Ok(ParseOutcome { files, warnings })
}

fn parse_count(lineno: usize, tag: &str, rest: &str) -> Result<u64, LcovError> {
    rest.trim()
        .parse()
        .map_err(|_| malformed(lineno, format!("{tag} has invalid count '{rest}'")))
}

/// Format: `FN:<start_line>[,<end_line>],<name>` - the end line (all digits)
/// is optional and function names may themselves contain commas.
fn parse_fn(rest: &str) -> Option<(u32, &str)> {
    let (start_field, after_start) = rest.split_once(',')?;
    let start_line: u32 = start_field.trim().parse().ok()?;
    let name = match after_start.split_once(',') {
        Some((maybe_end, tail))
            if !maybe_end.trim().is_empty()
                && maybe_end.trim().chars().all(|c| c.is_ascii_digit()) =>
        {
            tail
        }
        _ => after_start,
    };
    let name = name.trim();
    (!name.is_empty()).then_some((start_line, name))
}

/// Format: `FNDA:<count>,<name>` - names may contain commas.
fn parse_fnda(rest: &str) -> Option<(u64, &str)> {
    let (count_field, name) = rest.split_once(',')?;
    let hits: u64 = count_field.trim().parse().ok()?;
    let name = name.trim();
    (!name.is_empty()).then_some((hits, name))
}

/// Format: `BRDA:<line>,<block>,<branch>,<taken>` - taken is the last field
/// (`-` means the decision point was never evaluated); branch expressions may
/// contain commas.
fn parse_brda(rest: &str) -> Option<(u32, String, String, Option<u64>)> {
    let mut fields = rest.splitn(3, ',');
    let line_no: u32 = fields.next()?.trim().parse().ok()?;
    let block_id = fields.next()?.trim();
    let remainder = fields.next()?;
    let (branch_id, taken_field) = remainder.rsplit_once(',')?;
    let branch_id = branch_id.trim();
    if line_no == 0 || block_id.is_empty() || branch_id.is_empty() {
        return None;
    }
    let taken_field = taken_field.trim();
    let taken = if taken_field == "-" {
        None
    } else {
        Some(taken_field.parse().ok()?)
    };
    Some((line_no, block_id.to_string(), branch_id.to_string(), taken))
}

fn finish_block(
    block: Block,
    opts: &ParseOptions<'_>,
    merged: &mut BTreeMap<String, FileAcc>,
    warnings: &mut Vec<String>,
) {
    let executable = block.hits.len() as u64;
    let covered = block.hits.values().filter(|h| **h > 0).count() as u64;
    validate_count(warnings, &block.raw_path, "LF", block.lf, executable);
    validate_count(warnings, &block.raw_path, "LH", block.lh, covered);

    let functions_found = block.functions.len() as u64;
    let functions_hit = block.functions.values().filter(|(_, h)| *h > 0).count() as u64;
    validate_count(warnings, &block.raw_path, "FNF", block.fnf, functions_found);
    validate_count(warnings, &block.raw_path, "FNH", block.fnh, functions_hit);

    let branches_found = block.branches.len() as u64;
    let branches_hit = block
        .branches
        .values()
        .filter(|taken| taken.is_some_and(|t| t > 0))
        .count() as u64;
    validate_count(warnings, &block.raw_path, "BRF", block.brf, branches_found);
    validate_count(warnings, &block.raw_path, "BRH", block.brh, branches_hit);

    match normalize_sf_path(&block.raw_path, opts.repo_root) {
        Some(path) => {
            let acc = merged.entry(path).or_default();
            for (line, hits) in block.hits {
                let slot = acc.hits.entry(line).or_insert(0);
                *slot = slot.saturating_add(hits);
            }
            acc.branches.extend(block.branches.into_iter().map(
                |((line, block_id, branch_id), taken)| BranchHit {
                    line,
                    block: block_id,
                    branch: branch_id,
                    taken,
                },
            ));
            acc.functions.extend(
                block
                    .functions
                    .into_iter()
                    .map(|(name, (line, hits))| FunctionHit { name, line, hits }),
            );
        }
        None => warnings.push(format!(
            "{}: path resolves outside repo root, dropped",
            block.raw_path
        )),
    }
}

fn validate_count(
    warnings: &mut Vec<String>,
    path: &str,
    tag: &str,
    declared: Option<u64>,
    actual: u64,
) {
    if let Some(declared) = declared
        && declared != actual
    {
        let records = match tag {
            "LF" => "DA lines",
            "LH" => "covered DA lines",
            "FNF" => "functions",
            "FNH" => "covered functions",
            "BRF" => "branches",
            _ => "covered branches",
        };
        warnings.push(format!(
            "{path}: {tag}={declared} disagrees with {actual} {records}"
        ));
    }
}

/// Normalizes an `SF:` path to a repo-root relative `/`-separated path.
///
/// Returns `None` when the path resolves outside the repo root (third-party
/// code such as stdlib or vendored dependencies).
fn normalize_sf_path(raw: &str, repo_root: &Path) -> Option<String> {
    let unified = raw.replace('\\', "/");
    let path = Path::new(&unified);
    let relative = if path.is_absolute() {
        path.strip_prefix(repo_root).ok()?
    } else {
        path
    };

    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(segment) => parts.push(segment.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_relative_and_dot_segments() {
        let root = Path::new("/repo");
        assert_eq!(
            normalize_sf_path("./pkg/app.py", root),
            Some("pkg/app.py".to_string())
        );
        assert_eq!(
            normalize_sf_path("pkg/sub/../app.py", root),
            Some("pkg/app.py".to_string())
        );
        assert_eq!(
            normalize_sf_path("pkg\\win\\style.py", root),
            Some("pkg/win/style.py".to_string())
        );
    }

    #[test]
    fn normalize_absolute_paths_against_root() {
        let root = Path::new("/repo");
        assert_eq!(
            normalize_sf_path("/repo/src/a.py", root),
            Some("src/a.py".to_string())
        );
        assert_eq!(normalize_sf_path("/usr/lib/python3/os.py", root), None);
    }

    #[test]
    fn normalize_rejects_escaping_root() {
        let root = Path::new("/repo");
        assert_eq!(normalize_sf_path("../outside.py", root), None);
        assert_eq!(normalize_sf_path(".", root), None);
    }

    #[test]
    fn parse_fn_supports_optional_end_line_and_comma_names() {
        assert_eq!(parse_fn("12,main"), Some((12, "main")));
        assert_eq!(parse_fn("12,40,main"), Some((12, "main")));
        assert_eq!(
            parse_fn("12,operator<(a, b)"),
            Some((12, "operator<(a, b)"))
        );
        assert_eq!(parse_fn("abc,main"), None);
        assert_eq!(parse_fn("12"), None);
    }

    #[test]
    fn parse_brda_supports_dash_and_comma_expressions() {
        assert_eq!(
            parse_brda("7,0,1,4"),
            Some((7, "0".to_string(), "1".to_string(), Some(4)))
        );
        assert_eq!(
            parse_brda("7,0,0,-"),
            Some((7, "0".to_string(), "0".to_string(), None))
        );
        assert_eq!(
            parse_brda("7,e0,cond(a, b),0"),
            Some((7, "e0".to_string(), "cond(a, b)".to_string(), Some(0)))
        );
        assert_eq!(parse_brda("7,0,1"), None);
        assert_eq!(parse_brda("x,0,1,2"), None);
    }

    #[test]
    fn parse_brda_rejects_zero_lines_and_empty_identifiers() {
        assert_eq!(parse_brda("0,0,1,1"), None);
        assert_eq!(parse_brda("1,,1,1"), None);
        assert_eq!(parse_brda("1, ,1,1"), None);
        assert_eq!(parse_brda("1,0,,1"), None);
        assert_eq!(parse_brda("1,0, ,-"), None);
    }
}
