//! LCOV parsing into badgers coverage models.
//!
//! MVP scope: `SF`, `DA`, `LF`, `LH`, `end_of_record`. Function and branch
//! records (`FN`, `FNDA`, `BRDA`, ...) are ignored.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use badge_rs_core::{FileCoverage, Language, LineHit};

#[derive(Debug)]
pub struct ParseOptions<'a> {
    /// Absolute repository root used to relativize absolute `SF:` paths.
    pub repo_root: &'a Path,
}

#[derive(Debug)]
pub struct ParseOutcome {
    /// Sorted by path; same-path records are merged with hits summed.
    pub files: Vec<FileCoverage>,
    /// Non-fatal issues: LF/LH mismatches, dropped out-of-root paths, etc.
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
    lf: Option<u64>,
    lh: Option<u64>,
}

pub fn parse_lcov(input: &str, opts: &ParseOptions<'_>) -> Result<ParseOutcome, LcovError> {
    let mut merged: BTreeMap<String, BTreeMap<u32, u64>> = BTreeMap::new();
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
                current = Some(Block {
                    raw_path: path.to_string(),
                    hits: BTreeMap::new(),
                    lf: None,
                    lh: None,
                });
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
        .map(|(path, hits)| {
            let language = Language::from_path(&path);
            FileCoverage::new(
                path,
                language,
                hits.into_iter()
                    .map(|(line, hits)| LineHit { line, hits })
                    .collect(),
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

fn finish_block(
    block: Block,
    opts: &ParseOptions<'_>,
    merged: &mut BTreeMap<String, BTreeMap<u32, u64>>,
    warnings: &mut Vec<String>,
) {
    let executable = block.hits.len() as u64;
    let covered = block.hits.values().filter(|h| **h > 0).count() as u64;
    if let Some(lf) = block.lf
        && lf != executable
    {
        warnings.push(format!(
            "{}: LF={lf} disagrees with {executable} DA lines",
            block.raw_path
        ));
    }
    if let Some(lh) = block.lh
        && lh != covered
    {
        warnings.push(format!(
            "{}: LH={lh} disagrees with {covered} covered DA lines",
            block.raw_path
        ));
    }
    match normalize_sf_path(&block.raw_path, opts.repo_root) {
        Some(path) => {
            let entry = merged.entry(path).or_default();
            for (line, hits) in block.hits {
                let slot = entry.entry(line).or_insert(0);
                *slot = slot.saturating_add(hits);
            }
        }
        None => warnings.push(format!(
            "{}: path resolves outside repo root, dropped",
            block.raw_path
        )),
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
}
