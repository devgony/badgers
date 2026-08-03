use std::path::Path;

use badge_rs_lcov::{LlvmJsonError, ParseOptions, enrich_mcdc_from_llvm_json, parse_lcov};

fn opts(root: &Path) -> ParseOptions<'_> {
    ParseOptions { repo_root: root }
}

fn parsed_llvm20_files() -> Vec<badge_rs_core::FileCoverage> {
    let lcov = include_str!("fixtures/llvm20_mcdc.lcov");
    parse_lcov(lcov, &opts(Path::new("/repo"))).unwrap().files
}

#[test]
fn enriches_files_from_real_llvm20_export() {
    let mut files = parsed_llvm20_files();
    let warnings = enrich_mcdc_from_llvm_json(
        include_str!("fixtures/llvm20_mcdc.json"),
        &opts(Path::new("/repo")),
        &mut files,
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    let file = &files[0];
    assert_eq!(file.path, "src/sample.c");
    assert_eq!(file.total_mcdc(), 5);
    assert_eq!(file.covered_mcdc(), 3);
    assert!(file.mcdc.iter().all(|hit| hit.sense == "c"));
    assert!(file.mcdc.iter().all(|hit| hit.group.starts_with("llvm:")));
    assert!(!file.mcdc.iter().any(|hit| hit.is_unreachable()));

    let groups: std::collections::BTreeSet<_> =
        file.mcdc.iter().map(|hit| hit.group.clone()).collect();
    assert_eq!(groups.len(), 2);
}

#[test]
fn parses_major3_schema_with_and_without_test_vectors() {
    let major3 = r#"{
      "type": "llvm.coverage.json.export",
      "version": "3.1.0",
      "data": [{
        "files": [{
          "filename": "/repo/src/a.c",
          "mcdc_records": [
            [1, 44, 1, 57, 4, 0, 0, 0, 5, [true, false]],
            [2, 10, 2, 20, 1, 1, 0, 0, 5, [true], []]
          ],
          "summary": {
            "lines": {"count": 2, "covered": 2, "percent": 100},
            "mcdc": {"count": 3, "covered": 2, "notcovered": 1, "percent": 66}
          }
        }]
      }]
    }"#;
    let mut files = parse_lcov(
        "SF:/repo/src/a.c\nDA:1,4\nDA:2,1\nend_of_record\n",
        &opts(Path::new("/repo")),
    )
    .unwrap()
    .files;
    let warnings =
        enrich_mcdc_from_llvm_json(major3, &opts(Path::new("/repo")), &mut files).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(files[0].total_mcdc(), 3);
    assert_eq!(files[0].covered_mcdc(), 2);
}

#[test]
fn skips_file_when_records_disagree_with_summary() {
    let folded = r#"{
      "type": "llvm.coverage.json.export",
      "version": "2.0.1",
      "data": [{
        "files": [{
          "filename": "/repo/src/a.c",
          "mcdc_records": [[1, 44, 1, 57, 0, 5, [true, false, false]]],
          "summary": {
            "lines": {"count": 1, "covered": 1, "percent": 100},
            "mcdc": {"count": 2, "covered": 1, "notcovered": 1, "percent": 50}
          }
        }]
      }]
    }"#;
    let mut files = parse_lcov(
        "SF:/repo/src/a.c\nDA:1,4\nend_of_record\n",
        &opts(Path::new("/repo")),
    )
    .unwrap()
    .files;
    let warnings =
        enrich_mcdc_from_llvm_json(folded, &opts(Path::new("/repo")), &mut files).unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("disagree"), "{warnings:?}");
    assert_eq!(files[0].total_mcdc(), 0);
}

#[test]
fn skips_file_when_line_summary_is_stale() {
    let stale = r#"{
      "type": "llvm.coverage.json.export",
      "version": "2.0.1",
      "data": [{
        "files": [{
          "filename": "/repo/src/a.c",
          "mcdc_records": [[1, 44, 1, 57, 0, 5, [true]]],
          "summary": {
            "lines": {"count": 9, "covered": 9, "percent": 100},
            "mcdc": {"count": 1, "covered": 1, "notcovered": 0, "percent": 100}
          }
        }]
      }]
    }"#;
    let mut files = parse_lcov(
        "SF:/repo/src/a.c\nDA:1,4\nend_of_record\n",
        &opts(Path::new("/repo")),
    )
    .unwrap()
    .files;
    let warnings =
        enrich_mcdc_from_llvm_json(stale, &opts(Path::new("/repo")), &mut files).unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("stale sidecar"), "{warnings:?}");
    assert_eq!(files[0].total_mcdc(), 0);
}

#[test]
fn warns_and_skips_unmatched_paths_and_unknown_versions() {
    let unmatched = r#"{
      "type": "llvm.coverage.json.export",
      "version": "2.0.1",
      "data": [{
        "files": [{
          "filename": "/repo/src/other.c",
          "mcdc_records": [],
          "summary": {
            "lines": {"count": 1, "covered": 1},
            "mcdc": {"count": 0, "covered": 0}
          }
        }]
      }]
    }"#;
    let mut files = parse_lcov(
        "SF:/repo/src/a.c\nDA:1,4\nend_of_record\n",
        &opts(Path::new("/repo")),
    )
    .unwrap()
    .files;
    let warnings =
        enrich_mcdc_from_llvm_json(unmatched, &opts(Path::new("/repo")), &mut files).unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("no matching file"), "{warnings:?}");

    let future = r#"{
      "type": "llvm.coverage.json.export",
      "version": "9.0.0",
      "data": [{"files": []}]
    }"#;
    let warnings =
        enrich_mcdc_from_llvm_json(future, &opts(Path::new("/repo")), &mut files).unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("not supported"), "{warnings:?}");
}

#[test]
fn rejects_mixing_with_gcc_mcdc_records() {
    let mut files = parse_lcov(
        "SF:/repo/src/a.c\nDA:1,4\nMCDC:1,2,t,1,0,0\nend_of_record\n",
        &opts(Path::new("/repo")),
    )
    .unwrap()
    .files;
    let err = enrich_mcdc_from_llvm_json(
        r#"{"type": "llvm.coverage.json.export", "version": "2.0.1", "data": []}"#,
        &opts(Path::new("/repo")),
        &mut files,
    )
    .unwrap_err();
    assert!(matches!(err, LlvmJsonError::MixedMcdcSources));
}

#[test]
fn rejects_non_llvm_documents_and_invalid_json() {
    let mut files = Vec::new();
    let err = enrich_mcdc_from_llvm_json(
        r#"{"type": "something.else", "version": "2.0.1", "data": []}"#,
        &opts(Path::new("/repo")),
        &mut files,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unexpected document type"));

    let err =
        enrich_mcdc_from_llvm_json("not json", &opts(Path::new("/repo")), &mut files).unwrap_err();
    assert!(err.to_string().contains("invalid JSON"));
}

#[test]
fn skips_malformed_records_with_warnings() {
    let malformed = r#"{
      "type": "llvm.coverage.json.export",
      "version": "2.0.1",
      "data": [{
        "files": [{
          "filename": "/repo/src/a.c",
          "mcdc_records": [
            [1, 44, 1, 57, 0, 7, [true]],
            [0, 44, 1, 57, 0, 5, [true]],
            [1, 44, 1, 57, 0, 5]
          ],
          "summary": {
            "lines": {"count": 1, "covered": 1},
            "mcdc": {"count": 0, "covered": 0}
          }
        }]
      }]
    }"#;
    let mut files = parse_lcov(
        "SF:/repo/src/a.c\nDA:1,4\nend_of_record\n",
        &opts(Path::new("/repo")),
    )
    .unwrap()
    .files;
    let warnings =
        enrich_mcdc_from_llvm_json(malformed, &opts(Path::new("/repo")), &mut files).unwrap();
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(warnings[0].contains("unexpected MCDC record kind 7"));
    assert!(warnings[1].contains("line is zero"));
    assert!(warnings[2].contains("6 fields"));
    assert_eq!(files[0].total_mcdc(), 0);
}
