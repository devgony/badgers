use std::path::Path;

use badge_rs_core::{BranchHit, FunctionHit, Language, LineHit};
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
fn parses_function_and_branch_records_and_ignores_da_checksum() {
    let input = include_str!("fixtures/mixed_records.lcov");
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(outcome.files.len(), 1);
    let file = &outcome.files[0];
    assert_eq!(file.executable_lines(), 3);
    assert_eq!(file.covered_lines(), 3);
    assert_eq!(file.line_hits[1], LineHit { line: 2, hits: 3 });

    assert_eq!(
        file.functions,
        vec![FunctionHit {
            name: "main".to_string(),
            line: 1,
            hits: 3,
        }]
    );
    assert_eq!(file.total_functions(), 1);
    assert_eq!(file.covered_functions(), 1);

    assert_eq!(
        file.branches,
        vec![
            BranchHit {
                line: 2,
                block: "0".to_string(),
                branch: "0".to_string(),
                taken: Some(1),
            },
            BranchHit {
                line: 2,
                block: "0".to_string(),
                branch: "1".to_string(),
                taken: Some(0),
            },
        ]
    );
    assert_eq!(file.total_branches(), 2);
    assert_eq!(file.covered_branches(), 1);
}

#[test]
fn merges_dash_branches_across_repeated_sf_blocks() {
    let input = "\
SF:a.dart
DA:1,0
BRDA:1,0,0,-
BRDA:1,0,1,-
end_of_record
SF:a.dart
DA:1,2
BRDA:1,0,0,2
BRDA:1,0,1,-
end_of_record
";
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let file = &outcome.files[0];
    assert_eq!(
        file.branches,
        vec![
            BranchHit {
                line: 1,
                block: "0".to_string(),
                branch: "0".to_string(),
                taken: Some(2),
            },
            BranchHit {
                line: 1,
                block: "0".to_string(),
                branch: "1".to_string(),
                taken: None,
            },
        ]
    );
    assert_eq!(file.covered_branches(), 1);
}

#[test]
fn warns_on_malformed_function_and_branch_records_without_failing() {
    let input = "\
SF:a.py
DA:1,1
FN:notaline,main
FNDA:notacount,main
BRDA:1,0,0
end_of_record
";
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert_eq!(outcome.files.len(), 1);
    assert!(outcome.files[0].functions.is_empty());
    assert!(outcome.files[0].branches.is_empty());
    assert_eq!(outcome.warnings.len(), 3);
    assert!(outcome.warnings[0].contains("malformed FN"));
    assert!(outcome.warnings[1].contains("malformed FNDA"));
    assert!(outcome.warnings[2].contains("malformed BRDA"));
}

#[test]
fn warns_on_function_and_branch_summary_mismatches() {
    let input = "\
SF:a.py
DA:1,1
FN:1,main
FNDA:0,main
FNF:2
FNH:1
BRDA:1,0,0,1
BRF:2
BRH:0
end_of_record
";
    let outcome = parse_lcov(input, &opts(Path::new("/repo"))).unwrap();
    assert_eq!(outcome.warnings.len(), 4);
    assert!(outcome.warnings[0].contains("FNF=2"));
    assert!(outcome.warnings[1].contains("FNH=1"));
    assert!(outcome.warnings[2].contains("BRF=2"));
    assert!(outcome.warnings[3].contains("BRH=0"));
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
