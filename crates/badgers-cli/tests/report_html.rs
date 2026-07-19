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
  "pr_number": null,
  "generated_at": "2026-07-19T00:00:00Z",
  "tool_versions": {{ "badgers": "0.1.0", "cargo_llvm_cov": null, "coverage_py": null }},
  "files": [{files_json}]
}}"#
    )
}

#[test]
fn report_html_renders_index_and_file_pages() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("report-html");
    let _ = std::fs::remove_dir_all(&dir);
    let repo_root = dir.join("repo");

    write(
        &repo_root.join("pkg/calc.py"),
        "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n",
    );

    let head = snapshot_json(
        r#"{ "path": "pkg/calc.py", "language": "python",
             "line_hits": [
               {"line": 1, "hits": 1}, {"line": 2, "hits": 1},
               {"line": 5, "hits": 1}, {"line": 6, "hits": 0}
             ] }"#,
    );
    let base = snapshot_json(
        r#"{ "path": "pkg/calc.py", "language": "python",
             "line_hits": [ {"line": 1, "hits": 1}, {"line": 2, "hits": 1} ] }"#,
    );
    let diff = "\
--- a/pkg/calc.py
+++ b/pkg/calc.py
@@ -4,0 +5,2 @@
+def sub(a, b):
+    return a - b
";
    write(&dir.join("head.json"), &head);
    write(&dir.join("base.json"), &base);
    write(&dir.join("changes.diff"), diff);

    let out = dir.join("report");
    Command::cargo_bin("badgers")
        .unwrap()
        .arg("report")
        .arg("html")
        .arg("--head")
        .arg(dir.join("head.json"))
        .arg("--base")
        .arg(dir.join("base.json"))
        .arg("--diff-file")
        .arg(dir.join("changes.diff"))
        .arg("--repo-root")
        .arg(&repo_root)
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("index.html"));

    let index = std::fs::read_to_string(out.join("index.html")).unwrap();
    assert!(index.contains("pkg/</span>"), "directory row for pkg/");
    assert!(
        index.contains(r#"<a href="file-0.html">calc.py</a>"#),
        "file row links with file name"
    );
    assert!(index.contains("75.00%"), "head total pct");
    assert!(index.contains("-25.00%p"), "delta vs base (100% -> 75%)");
    assert!(index.contains("1/2 (50.00%)"), "diff coverage cell");

    let page = std::fs::read_to_string(out.join("file-0.html")).unwrap();
    assert!(page.contains(r#"<tr id="L5" class="cov changed">"#));
    assert!(page.contains(r#"<tr id="L6" class="miss changed">"#));
    assert!(page.contains(r#"<tr id="L1" class="cov">"#));
    assert!(page.contains("Uncovered changed lines"));
    assert!(page.contains("def sub(a, b):"));
}

#[test]
fn report_html_labels_empty_and_comment_only_files() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("report-html-empty");
    let _ = std::fs::remove_dir_all(&dir);
    let repo_root = dir.join("repo");

    write(&repo_root.join("pkg/__init__.py"), "");
    write(
        &repo_root.join("pkg/notes.py"),
        "# comment only\n\n# more\n",
    );

    let head = snapshot_json(
        r#"{ "path": "pkg/__init__.py", "language": "python", "line_hits": [] },
           { "path": "pkg/notes.py", "language": "python", "line_hits": [] }"#,
    );
    write(&dir.join("head.json"), &head);

    let out = dir.join("report");
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["report", "html"])
        .arg("--head")
        .arg(dir.join("head.json"))
        .arg("--repo-root")
        .arg(&repo_root)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();

    let index = std::fs::read_to_string(out.join("index.html")).unwrap();
    assert_eq!(index.matches("no executable lines").count(), 2);

    let init_page = std::fs::read_to_string(out.join("file-0.html")).unwrap();
    assert!(init_page.contains("This file is empty"));
    assert!(init_page.contains("empty file"));
    assert!(!init_page.contains("<table class=\"code\">"));

    let notes_page = std::fs::read_to_string(out.join("file-1.html")).unwrap();
    assert!(notes_page.contains("No executable lines"));
    assert!(notes_page.contains("# comment only"));
}

#[test]
fn report_html_works_without_base_and_diff() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("report-html-nobase");
    let _ = std::fs::remove_dir_all(&dir);
    let repo_root = dir.join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();

    let head = snapshot_json(
        r#"{ "path": "pkg/missing_source.py", "language": "python",
             "line_hits": [ {"line": 1, "hits": 1} ] }"#,
    );
    write(&dir.join("head.json"), &head);

    let out = dir.join("report");
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["report", "html"])
        .arg("--head")
        .arg(dir.join("head.json"))
        .arg("--repo-root")
        .arg(&repo_root)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();

    let index = std::fs::read_to_string(out.join("index.html")).unwrap();
    assert!(index.contains("no base snapshot"));
    let page = std::fs::read_to_string(out.join("file-0.html")).unwrap();
    assert!(page.contains("source not available"));
}
