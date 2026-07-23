use assert_cmd::Command;
use predicates::prelude::*;

fn fixture_path() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/flutter_basic.lcov"
    )
}

#[test]
fn collect_flutter_from_lcov_fixture_writes_snapshot() {
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("flutter-snapshot.json");

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "flutter", "--lcov-file", fixture_path()])
        .args(["--repo-root", "."])
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("TOTAL"))
        .stdout(predicate::str::contains("lib/app.dart"))
        .stdout(predicate::str::contains("66.67%"));

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(json["schema_version"], 1);
    let files = json["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"], "lib/app.dart");
    assert_eq!(files[0]["language"], "dart");
    assert_eq!(files[0]["line_hits"].as_array().unwrap().len(), 4);
    assert!(json["generated_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(json["tool_versions"]["badgers"], env!("CARGO_PKG_VERSION"));
    assert!(json["tool_versions"]["flutter"].is_null());
}

#[test]
fn collect_flutter_reads_default_coverage_lcov_info() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join("coverage")).unwrap();
    std::fs::copy(fixture_path(), repo.path().join("coverage/lcov.info")).unwrap();
    let out = repo.path().join("snapshot.json");

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "flutter"])
        .arg("--repo-root")
        .arg(repo.path())
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("lib/app.dart"));

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
}

#[test]
fn collect_flutter_missing_default_file_mentions_flutter_test() {
    let repo = tempfile::tempdir().unwrap();

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "flutter"])
        .arg("--repo-root")
        .arg(repo.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("flutter test --coverage"));
}

#[test]
fn collect_flutter_missing_lcov_file_fails_with_code_1() {
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "flutter", "--lcov-file", "does-not-exist.lcov"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("failed to read LCOV file"));
}
