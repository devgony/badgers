use assert_cmd::Command;
use predicates::prelude::*;

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn snapshot_json(files_json: &str) -> String {
    format!(
        r#"{{
  "schema_version": 1,
  "repo": "owner/repo",
  "commit_sha": "abcdef1234567890",
  "branch": null,
  "pr_number": 42,
  "generated_at": "2026-07-19T00:00:00Z",
  "tool_versions": {{ "badgers": "0.1.1", "cargo_llvm_cov": null, "coverage_py": "7.6" }},
  "files": [{files_json}]
}}"#
    )
}

#[test]
fn report_markdown_renders_hierarchy_and_source_links() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("report-markdown");
    let _ = std::fs::remove_dir_all(&dir);

    let head = snapshot_json(
        r#"{ "path": "apps/api/calc.py", "language": "python",
             "line_hits": [
               {"line": 1, "hits": 1}, {"line": 2, "hits": 1},
               {"line": 5, "hits": 1}, {"line": 6, "hits": 0}
             ] },
           { "path": "README.md", "language": "unknown", "line_hits": [] }"#,
    );
    let base = snapshot_json(
        r#"{ "path": "apps/api/calc.py", "language": "python",
             "line_hits": [ {"line": 1, "hits": 1}, {"line": 2, "hits": 1} ] },
           { "path": "README.md", "language": "unknown", "line_hits": [] }"#,
    );
    let diff = "\
--- a/apps/api/calc.py
+++ b/apps/api/calc.py
@@ -4,0 +5,2 @@
+def sub(a, b):
+    return a - b
";
    write(&dir.join("head.json"), &head);
    write(&dir.join("base.json"), &base);
    write(&dir.join("changes.diff"), diff);

    let output = dir.join("coverage-summary.md");
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["report", "markdown"])
        .arg("--head")
        .arg(dir.join("head.json"))
        .arg("--base")
        .arg(dir.join("base.json"))
        .arg("--diff-file")
        .arg(dir.join("changes.diff"))
        .arg("--source-url")
        .arg("https://github.example/owner/repo/blob/abcdef1")
        .arg("--output")
        .arg(&output)
        .assert()
        .success()
        .stdout(predicate::str::contains("coverage-summary.md"));

    let markdown = std::fs::read_to_string(output).unwrap();
    assert!(markdown.contains("# 🦡 Badgers Coverage Report"));
    assert!(markdown.contains("<summary>📁 <strong>apps/</strong>"));
    assert!(markdown.contains("<summary>📁 <strong>api/</strong>"));
    assert!(markdown.contains("75.00% (3/4)"));
    assert!(markdown.contains("🔴 -25.00%p"));
    assert!(markdown.contains("50.00% (1/2)"));
    assert!(markdown.contains(
        "<a href=\"https://github.example/owner/repo/blob/abcdef1/apps/api/calc.py\"><code>apps/api/calc.py</code></a>"
    ));
    assert!(markdown.contains(
        "<a href=\"https://github.example/owner/repo/blob/abcdef1/apps/api/calc.py#L6\">L6</a>"
    ));
}

#[test]
fn report_markdown_uses_snapshot_source_url_without_baseline() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("report-markdown-no-base");
    let _ = std::fs::remove_dir_all(&dir);
    let head = snapshot_json(
        r#"{ "path": "pkg/my file.py", "language": "python",
             "line_hits": [ {"line": 1, "hits": 1} ] }"#,
    );
    write(&dir.join("head.json"), &head);

    let output = dir.join("coverage-summary.md");
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["report", "markdown"])
        .arg("--head")
        .arg(dir.join("head.json"))
        .arg("--output")
        .arg(&output)
        .assert()
        .success();

    let markdown = std::fs::read_to_string(output).unwrap();
    assert!(markdown.contains("n/a (no baseline)"));
    assert!(
        markdown.contains("https://github.com/owner/repo/blob/abcdef1234567890/pkg/my%20file.py")
    );
    assert!(markdown.contains("No measurable changed lines."));
}

#[test]
fn report_markdown_rejects_unsafe_source_url() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("report-markdown-url");
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir.join("head.json"), &snapshot_json(""));

    Command::cargo_bin("badgers")
        .unwrap()
        .args(["report", "markdown"])
        .arg("--head")
        .arg(dir.join("head.json"))
        .arg("--source-url")
        .arg("javascript:alert(1)")
        .arg("--output")
        .arg(dir.join("summary.md"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("source URL must use https://"));
}

#[test]
fn report_markdown_does_not_link_removed_files_to_head() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("report-markdown-removed");
    let _ = std::fs::remove_dir_all(&dir);
    let head = snapshot_json("");
    let base = snapshot_json(
        r#"{ "path": "pkg/old.py", "language": "python",
             "line_hits": [ {"line": 1, "hits": 1} ] }"#,
    );
    write(&dir.join("head.json"), &head);
    write(&dir.join("base.json"), &base);

    let output = dir.join("coverage-summary.md");
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["report", "markdown"])
        .arg("--head")
        .arg(dir.join("head.json"))
        .arg("--base")
        .arg(dir.join("base.json"))
        .arg("--output")
        .arg(&output)
        .assert()
        .success();

    let markdown = std::fs::read_to_string(output).unwrap();
    assert!(markdown.contains("<code>pkg/old.py</code>"));
    assert!(markdown.contains("| — | — | removed |"));
    assert!(
        !markdown.contains("href=\"https://github.com/owner/repo/blob/abcdef1234567890/pkg/old.py")
    );
}
