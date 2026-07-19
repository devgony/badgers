use assert_cmd::Command;
use predicates::prelude::*;

fn fixture_path() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/python_basic.lcov"
    )
}

#[test]
fn collect_python_from_lcov_fixture_writes_snapshot() {
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("snapshot.json");

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "python", "--lcov-file", fixture_path()])
        .args(["--repo-root", "."])
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("TOTAL"))
        .stdout(predicate::str::contains("pkg/app.py"))
        .stdout(predicate::str::contains("66.67%"));

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(json["schema_version"], 1);
    let files = json["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"], "pkg/app.py");
    assert_eq!(files[0]["language"], "python");
    assert_eq!(files[0]["line_hits"].as_array().unwrap().len(), 4);
    assert!(json["generated_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(json["tool_versions"]["badgers"], env!("CARGO_PKG_VERSION"));
    assert!(json["tool_versions"]["coverage_py"].is_null());
}

#[test]
fn collect_python_missing_lcov_file_fails_with_code_1() {
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "python", "--lcov-file", "does-not-exist.lcov"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("failed to read LCOV file"));
}

#[test]
fn usage_error_exits_with_code_2() {
    Command::cargo_bin("badgers")
        .unwrap()
        .arg("collect")
        .assert()
        .code(2);
}
