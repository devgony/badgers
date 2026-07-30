use assert_cmd::Command;
use predicates::prelude::*;

fn python_fixture() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/python_basic.lcov"
    )
}

fn flutter_fixture() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/flutter_basic.lcov"
    )
}

#[test]
fn collect_lcov_from_python_fixture_writes_snapshot() {
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("python-snapshot.json");

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "lcov", "--lcov-file", python_fixture()])
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
fn collect_lcov_classifies_dart_files() {
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("flutter-snapshot.json");

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "lcov", "--lcov-file", flutter_fixture()])
        .args(["--repo-root", "."])
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("lib/app.dart"));

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(json["files"][0]["path"], "lib/app.dart");
    assert_eq!(json["files"][0]["language"], "dart");
}

#[test]
fn collect_lcov_defaults_to_coverage_lcov_info() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join("coverage")).unwrap();
    std::fs::copy(flutter_fixture(), repo.path().join("coverage/lcov.info")).unwrap();
    let out = repo.path().join("snapshot.json");

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "lcov"])
        .arg("--repo-root")
        .arg(repo.path())
        .arg("-o")
        .arg(&out)
        .current_dir(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("lib/app.dart"));

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
}

#[test]
fn collect_lcov_captures_branch_and_function_coverage() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/branch_function.lcov"
    );
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("branch-snapshot.json");

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "lcov", "--lcov-file", fixture])
        .args(["--repo-root", "."])
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("Branch coverage:   50.00% (2/4)"))
        .stdout(predicate::str::contains("Function coverage: 50.00% (1/2)"));

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let file = &json["files"][0];
    assert_eq!(file["path"], "lib/calc.dart");
    let branches = file["branches"].as_array().unwrap();
    assert_eq!(branches.len(), 4);
    assert_eq!(branches[0]["line"], 2);
    assert_eq!(branches[0]["taken"], 3);
    assert!(branches[2]["taken"].is_null());
    let functions = file["functions"].as_array().unwrap();
    assert_eq!(functions.len(), 2);
    assert_eq!(functions[0]["name"], "add");
    assert_eq!(functions[0]["hits"], 4);
}

#[test]
fn collect_lcov_missing_file_fails_with_code_1() {
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "lcov", "--lcov-file", "does-not-exist.lcov"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("failed to read LCOV file"))
        .stderr(predicate::str::contains(
            "did your coverage command write it?",
        ));
}

#[test]
fn usage_error_exits_with_code_2() {
    Command::cargo_bin("badgers")
        .unwrap()
        .arg("collect")
        .assert()
        .code(2);
}

#[test]
fn collect_prefers_checked_out_commit_over_github_merge_sha() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join("tracked.txt"), "fixture\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Badgers Test"],
        vec!["config", "user.email", "badgers@example.com"],
        vec!["add", "tracked.txt"],
        vec!["commit", "-q", "-m", "fixture"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success()
        );
    }
    let head = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let out = repo.path().join("snapshot.json");

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["collect", "lcov", "--lcov-file", python_fixture()])
        .arg("--repo-root")
        .arg(repo.path())
        .arg("--output")
        .arg(&out)
        .env("GITHUB_SHA", "0000000000000000000000000000000000000000")
        .assert()
        .success();

    let snapshot: serde_json::Value = serde_json::from_slice(&std::fs::read(out).unwrap()).unwrap();
    assert_eq!(snapshot["commit_sha"], head.trim());
}
