use std::collections::{BTreeMap, BTreeSet};

use crate::compare::ChangedLines;

/// Extracts added/modified line numbers (new-file side) per path from
/// unified diff text, at any context width.
///
/// Hunk bodies are walked so that only `+` lines count as changed. Trusting the
/// `@@ -<old> +<new_start>[,<new_count>] @@` range instead would also mark the
/// surrounding context, which matters because GitHub's API only ever serves
/// diffs with three lines of context.
pub fn parse_unified_diff(text: &str) -> ChangedLines {
    let mut map: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut new_line: Option<u32> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let target = rest.split('\t').next().unwrap_or(rest).trim();
            current = if target == "/dev/null" {
                None
            } else {
                Some(target.strip_prefix("b/").unwrap_or(target).to_string())
            };
            new_line = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            // A pure deletion starts the new side at 0, leaving no line to mark.
            new_line = parse_new_start(rest).filter(|start| *start > 0);
            continue;
        }
        let (Some(path), Some(cursor)) = (current.as_ref(), new_line.as_mut()) else {
            continue;
        };
        match line.as_bytes().first() {
            Some(b'+') => {
                map.entry(path.clone()).or_default().insert(*cursor);
                *cursor += 1;
            }
            Some(b'-') => {}
            // "\ No newline at end of file" annotates the previous line.
            Some(b'\\') => {}
            _ => *cursor += 1,
        }
    }

    ChangedLines(map)
}

fn parse_new_start(hunk: &str) -> Option<u32> {
    let plus_field = hunk
        .split_whitespace()
        .find(|field| field.starts_with('+'))?;
    let spec = &plus_field[1..];
    let start = spec.split_once(',').map_or(spec, |(start, _)| start);
    start.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_added_and_modified_hunks() {
        let diff = "\
diff --git a/pkg/calc.py b/pkg/calc.py
index 111..222 100644
--- a/pkg/calc.py
+++ b/pkg/calc.py
@@ -10,0 +11,3 @@ def classify(n):
+    if n == 0:
+        return \"zero\"
+    return None
@@ -20 +23 @@ def other():
-    old
+    new
";
        let changed = parse_unified_diff(diff);
        let lines = changed.for_path("pkg/calc.py").unwrap();
        assert_eq!(
            lines.iter().copied().collect::<Vec<_>>(),
            vec![11, 12, 13, 23]
        );
    }

    #[test]
    fn skips_deleted_files_and_pure_deletions() {
        let diff = "\
--- a/gone.py
+++ /dev/null
@@ -1,3 +0,0 @@
-a
-b
-c
--- a/kept.py
+++ b/kept.py
@@ -5,2 +5,0 @@
-x
-y
";
        let changed = parse_unified_diff(diff);
        assert!(changed.for_path("gone.py").is_none());
        assert!(changed.for_path("kept.py").is_none());
    }

    #[test]
    fn handles_no_prefix_paths() {
        let diff = "\
--- kept.py
+++ kept.py
@@ -1 +1,2 @@
+added
 ctx
";
        let changed = parse_unified_diff(diff);
        assert_eq!(
            changed
                .for_path("kept.py")
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn ignores_context_lines_around_additions() {
        let diff = "\
diff --git a/pkg/calc.py b/pkg/calc.py
--- a/pkg/calc.py
+++ b/pkg/calc.py
@@ -1,5 +1,6 @@
 one
 two
+inserted
 three
 four
 five
";
        let changed = parse_unified_diff(diff);
        assert_eq!(
            changed
                .for_path("pkg/calc.py")
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn tracks_new_side_across_deletions_and_no_newline_markers() {
        let diff = "\
--- a/pkg/calc.py
+++ b/pkg/calc.py
@@ -1,6 +1,5 @@
 keep
-removed
-also removed
+replacement
 tail
\\ No newline at end of file
+appended
";
        let changed = parse_unified_diff(diff);
        assert_eq!(
            changed
                .for_path("pkg/calc.py")
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![2, 4]
        );
    }

    #[test]
    fn parses_multiple_files_in_one_diff() {
        let diff = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -10,3 +10,4 @@ fn a() {
 ctx
+added_in_a
 ctx
 ctx
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,2 +1,3 @@
 ctx
+added_in_b
 ctx
";
        let changed = parse_unified_diff(diff);
        assert_eq!(
            changed
                .for_path("a.rs")
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![11]
        );
        assert_eq!(
            changed
                .for_path("b.rs")
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![2]
        );
    }
}
