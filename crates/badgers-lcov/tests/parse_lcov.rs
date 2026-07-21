use std::path::Path;

use badge_rs_core::{Language, LineHit};
use badge_rs_lcov::{LcovError, ParseOptions, parse_lcov};

fn opts(root: &Path) -> ParseOptions<'_> {
    ParseOptions { repo_root: root }
}

#[test]
fn parses_basic_python_lcov() {
    let input = include_str!("fixtures/python_basic.lcov");
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(outcome.files.len(), 2);

    let app = &outcome.files[0];
    assert_eq!(app.path, "pkg/app.py");
    assert_eq!(app.language, Language::Python);
    assert_eq!(app.executable_lines(), 4);
    assert_eq!(app.covered_lines(), 3);
    assert_eq!(
        app.line_hits,
        vec![
            LineHit { line: 1, hits: 1 },
            LineHit { line: 2, hits: 1 },
            LineHit { line: 3, hits: 0 },
            LineHit { line: 5, hits: 4 },
        ]
    );

    let util = &outcome.files[1];
    assert_eq!(util.path, "pkg/util.py");
    assert_eq!(util.executable_lines(), 2);
    assert_eq!(util.covered_lines(), 1);
}

#[test]
fn ignores_function_and_branch_records_and_da_checksum() {
    let input = include_str!("fixtures/mixed_records.lcov");
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(outcome.files.len(), 1);
    let file = &outcome.files[0];
    assert_eq!(file.executable_lines(), 3);
    assert_eq!(file.covered_lines(), 3);
    assert_eq!(file.line_hits[1], LineHit { line: 2, hits: 3 });
}

#[test]
fn merges_duplicate_da_lines_and_repeated_sf_blocks() {
    let input = "\
SF:a.py
DA:1,1
DA:1,2
DA:2,0
end_of_record
SF:a.py
DA:2,5
end_of_record
";
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert_eq!(outcome.files.len(), 1);
    assert_eq!(
        outcome.files[0].line_hits,
        vec![LineHit { line: 1, hits: 3 }, LineHit { line: 2, hits: 5 }]
    );
}

#[test]
fn warns_on_lf_lh_mismatch() {
    let input = "\
SF:a.py
DA:1,1
LF:2
LH:0
end_of_record
";
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert_eq!(outcome.warnings.len(), 2);
    assert!(outcome.warnings[0].contains("LF=2"));
    assert!(outcome.warnings[1].contains("LH=0"));
}

#[test]
fn normalizes_absolute_paths_and_drops_out_of_root() {
    let input = "\
SF:/repo/src/inside.py
DA:1,1
end_of_record
SF:/usr/lib/python3.14/os.py
DA:1,9
end_of_record
";
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert_eq!(outcome.files.len(), 1);
    assert_eq!(outcome.files[0].path, "src/inside.py");
    assert_eq!(outcome.warnings.len(), 1);
    assert!(outcome.warnings[0].contains("outside repo root"));
}

#[test]
fn errors_on_da_before_sf() {
    let err = parse_lcov("DA:1,1\n", &opts(Path::new("/repo"))).unwrap_err();
    let LcovError::Malformed { line, message } = err;
    assert_eq!(line, 1);
    assert!(message.contains("DA before SF"));
}

#[test]
fn errors_on_malformed_da() {
    let input = "SF:a.py\nDA:abc,1\nend_of_record\n";
    let err = parse_lcov(input, &opts(Path::new("/repo"))).unwrap_err();
    let LcovError::Malformed { line, .. } = err;
    assert_eq!(line, 2);
}

#[test]
fn errors_on_unterminated_record() {
    let input = "SF:a.py\nDA:1,1\n";
    let err = parse_lcov(input, &opts(Path::new("/repo"))).unwrap_err();
    let LcovError::Malformed { message, .. } = err;
    assert!(message.contains("unterminated record"));
}

#[test]
fn errors_on_end_of_record_without_sf() {
    let err = parse_lcov("end_of_record\n", &opts(Path::new("/repo"))).unwrap_err();
    let LcovError::Malformed { message, .. } = err;
    assert!(message.contains("without preceding SF"));
}
