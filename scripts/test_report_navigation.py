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


if __name__ == "__main__":
    _ = unittest.main()
