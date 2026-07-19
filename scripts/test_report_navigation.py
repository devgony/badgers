from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from . import report_navigation


SCRIPT = Path(__file__).with_name("report_navigation.py")


def base_env() -> dict[str, str]:
    return {
        "GITHUB_SERVER_URL": "https://github.example",
        "BADGERS_SOURCE_REPO": "source/project",
        "BADGERS_PR_HEAD_REPO": "source/project",
        "BADGERS_PR_NUMBER": "42",
        "BADGERS_STORAGE_REPO": "reports/archive",
        "BADGERS_STORAGE_BRANCH": "coverage/reports",
        "BADGERS_STORAGE_PREFIX": "/badgers/history/",
    }


class ReportNavigationTests(unittest.TestCase):
    def test_happy_path_and_encoding(self) -> None:
        files, durable = report_navigation.build_urls(base_env())
        self.assertEqual(files, "https://github.example/source/project/pull/42/files")
        self.assertEqual(
            durable,
            "https://github.example/reports/archive/blob/coverage%2Freports/"
            + "badgers/history/repos/source/project/prs/42/README.md",
        )

    def test_storage_disabled_keeps_durable_url_empty(self) -> None:
        env = base_env()
        env["BADGERS_STORAGE_REPO"] = ""
        self.assertEqual(report_navigation.build_urls(env)[1], "")

    def test_fork_keeps_durable_url_empty(self) -> None:
        env = base_env()
        env["BADGERS_PR_HEAD_REPO"] = "fork/project"
        self.assertEqual(report_navigation.build_urls(env)[1], "")

    def test_empty_prefix_omits_prefix_segment(self) -> None:
        env = base_env()
        env["BADGERS_STORAGE_PREFIX"] = ""
        self.assertEqual(
            report_navigation.build_urls(env)[1],
            "https://github.example/reports/archive/blob/coverage%2Freports/"
            + "repos/source/project/prs/42/README.md",
        )

    def test_encodes_storage_path_segments(self) -> None:
        env = base_env()
        env["BADGERS_STORAGE_PREFIX"] = "badgers reports/100%"
        self.assertIn(
            "/badgers%20reports/100%25/repos/",
            report_navigation.build_urls(env)[1],
        )

    def test_rejects_unsafe_inputs(self) -> None:
        cases = [
            ("GITHUB_SERVER_URL", "http://github.example"),
            ("GITHUB_SERVER_URL", "https://user@github.example"),
            ("BADGERS_SOURCE_REPO", "source/../project"),
            ("BADGERS_PR_HEAD_REPO", "fork\\project"),
            ("BADGERS_STORAGE_REPO", "reports/project/extra"),
            ("BADGERS_STORAGE_PREFIX", "badgers/../escape"),
            ("BADGERS_STORAGE_PREFIX", "badgers\\..\\escape"),
            ("BADGERS_PR_NUMBER", "42\nname=value"),
            ("BADGERS_PR_NUMBER", "０１"),
        ]
        for key, value in cases:
            with self.subTest(key=key, value=value):
                env = base_env()
                env[key] = value
                with self.assertRaises(ValueError):
                    _ = report_navigation.build_urls(env)

    def test_main_appends_github_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory, "output")
            _ = output.write_text("existing=value\n", encoding="utf-8")
            env = os.environ.copy()
            env.update(base_env())
            env["GITHUB_OUTPUT"] = str(output)
            _ = subprocess.run([sys.executable, str(SCRIPT)], env=env, check=True)
            contents = output.read_text(encoding="utf-8")
        self.assertTrue(contents.startswith("existing=value\n"))
        self.assertIn("files-changed-url=https://github.example/source/project/pull/42/files\n", contents)
        self.assertIn("durable-report-url=https://github.example/reports/archive/", contents)

    def test_action_invokes_tested_helper_and_passes_fork_identity(self) -> None:
        action = SCRIPT.parent.parent.joinpath("action.yml").read_text(encoding="utf-8")
        self.assertIn('python3 "$GITHUB_ACTION_PATH/scripts/report_navigation.py"', action)
        self.assertIn("BADGERS_PR_HEAD_REPO: ${{ github.event.pull_request.head.repo.full_name }}", action)
        self.assertNotIn("python - \"$GITHUB_OUTPUT\" <<'PY'", action)
        self.assertNotIn("https://x-access-token:${GH_TOKEN}@", action)
        self.assertGreaterEqual(
            action.count("github.event.pull_request.head.repo.full_name == github.repository"),
            5,
        )

        lines = action.splitlines()
        run_blocks: list[str] = []
        index = 0
        while index < len(lines):
            if lines[index].startswith("      run: |"):
                index += 1
                block: list[str] = []
                while index < len(lines) and (
                    not lines[index].strip() or lines[index].startswith("        ")
                ):
                    block.append(lines[index][8:] if lines[index] else "")
                    index += 1
                run_blocks.append("\n".join(block))
                continue
            index += 1
        self.assertTrue(run_blocks)
        for script_body in run_blocks:
            self.assertNotIn("${{", script_body)
            _ = subprocess.run(["bash", "-n", "-c", script_body], check=True)


if __name__ == "__main__":
    _ = unittest.main()
