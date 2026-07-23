from __future__ import annotations

import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HELPERS = "source scripts/release_helpers.sh"


def _bash(script: str, *, standard_input: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["bash", "-c", f"set -euo pipefail; {HELPERS}; {script}"],
        cwd=ROOT,
        input=standard_input,
        text=True,
        capture_output=True,
        check=False,
    )


class ReleaseHelperTests(unittest.TestCase):
    def test_annotated_tag_resolves_to_peeled_commit(self) -> None:
        result = _bash(
            "tag_commit_from_ls_remote v1.2.3",
            standard_input=(
                "aaaaaaaa\trefs/tags/v1.2.3\n"
                "bbbbbbbb\trefs/tags/v1.2.3^{}\n"
            ),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "bbbbbbbb")

    def test_lightweight_tag_resolves_to_direct_commit(self) -> None:
        result = _bash(
            "tag_commit_from_ls_remote v1.2.3",
            standard_input="aaaaaaaa\trefs/tags/v1.2.3\n",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "aaaaaaaa")

    def test_release_create_error_is_reconciled_when_release_exists(self) -> None:
        result = _bash(
            """
            gh() {
              if [[ "$2" == create ]]; then return 42; fi
              return 0
            }
            create_release_reconciled v1.2.3
            """
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("release now exists", result.stdout)

    def test_release_create_error_remains_failure_when_release_is_absent(self) -> None:
        result = _bash(
            """
            gh() { return 42; }
            create_release_reconciled v1.2.3
            """
        )
        self.assertEqual(result.returncode, 42)
        self.assertIn("rerun make release to resume", result.stderr)

    def test_major_tag_push_error_is_reconciled_when_remote_advanced(self) -> None:
        result = _bash(
            """
            git() {
              if [[ "$1" == push ]]; then return 42; fi
              if [[ "$1" == tag ]]; then return 0; fi
              if [[ "$2" == --tags ]]; then
                printf 'release-commit\trefs/tags/v1\n'
              fi
            }
            advance_major_tag 1.2.3 release-commit
            """
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("origin contains v1", result.stdout)


class ReleaseFlowStaticTests(unittest.TestCase):
    def test_recovery_precedes_bump_selection_and_reuses_current_version(self) -> None:
        script = (ROOT / "scripts/release.sh").read_text(encoding="utf-8")
        recovery = script.index('remote_current_tag=$(remote_tag_commit "$current_tag"')
        selection = script.index("bump=${BUMP:-}")
        self.assertLess(recovery, selection)
        self.assertIn(
            'git merge-base --is-ancestor "$remote_current_tag" "$head"', script
        )
        self.assertIn('create_release_reconciled "$current_tag"', script)
        self.assertIn(
            'advance_major_tag "$current_version" "$remote_current_tag"', script
        )
        self.assertIn('remote_major_tag=$(remote_tag_commit "$current_major_tag"', script)

    def test_ambiguous_push_checks_both_remote_refs_before_continuing(self) -> None:
        script = (ROOT / "scripts/release.sh").read_text(encoding="utf-8")
        self.assertIn("remote_main=$(remote_main_commit || true)", script)
        self.assertIn('remote_tag=$(remote_tag_commit "$tag" || true)', script)
        self.assertIn(
            '[[ "$remote_main" == "$release_commit" && "$remote_tag" == "$release_commit" ]]',
            script,
        )
        self.assertIn("cleanup_allowed=false", script)

    def test_workflow_validates_once_and_downstream_jobs_checkout_sha(self) -> None:
        workflow = (ROOT / ".github/workflows/release-cli.yml").read_text(
            encoding="utf-8"
        )
        sha_checkout = "ref: ${{ needs.validate.outputs.sha }}"
        validated_tag = "BADGERS_RELEASE_TAG: ${{ needs.validate.outputs.tag }}"
        self.assertEqual(workflow.count("\n      REQUESTED_TAG:"), 1)
        self.assertEqual(workflow.count(sha_checkout), 3)
        self.assertEqual(workflow.count(validated_tag), 3)
        self.assertEqual(workflow.count("needs: validate"), 2)
        self.assertIn("needs: [validate, build]", workflow)
        self.assertIn('git rev-parse "refs/tags/$REQUESTED_TAG^{commit}"', workflow)
        self.assertIn('gh release view "$REQUESTED_TAG"', workflow)
        self.assertIn('[[ "$REQUESTED_TAG" != "v$VERSION" ]]', workflow)


if __name__ == "__main__":
    _ = unittest.main()
