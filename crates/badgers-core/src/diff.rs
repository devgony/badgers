use std::collections::{BTreeMap, BTreeSet};

use crate::compare::ChangedLines;

/// Extracts added/modified line numbers (new-file side) per path from
/// unified diff text (`git diff --unified=0 base...head` recommended).
///
/// Hunk header format: `@@ -<old_start>[,<old_count>] +<new_start>[,<new_count>] @@`.
/// A missing count defaults to 1; `new_count == 0` (pure deletion) adds nothing.
pub fn parse_unified_diff(text: &str) -> ChangedLines {
    let mut map: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    let mut current: Option<String> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let target = rest.split('\t').next().unwrap_or(rest).trim();
            current = if target == "/dev/null" {
                None
            } else {
                Some(target.strip_prefix("b/").unwrap_or(target).to_string())
            };
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            let Some(path) = &current else { continue };
            let Some((start, count)) = parse_new_side(rest) else {
                continue;
            };
            if count == 0 {
                continue;
            }
            let entry = map.entry(path.clone()).or_default();
            entry.extend(start..start.saturating_add(count));
        }
    }

    ChangedLines(map)
}

fn parse_new_side(hunk: &str) -> Option<(u32, u32)> {
    let plus_field = hunk
        .split_whitespace()
        .find(|field| field.starts_with('+'))?;
    let spec = &plus_field[1..];
    match spec.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((spec.parse().ok()?, 1)),
    }
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
            vec![1, 2]
        );
    }
}
