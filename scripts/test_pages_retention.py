from __future__ import annotations

import shutil
import tempfile
import unittest
from pathlib import Path

from . import pages_retention


def _make_site(*names: str) -> Path:
    site = Path(tempfile.mkdtemp())
    for name in names:
        directory = site / name
        directory.mkdir()
        _ = (directory / "index.html").write_text("<html></html>", encoding="utf-8")
    return site


class PagesRetentionTests(unittest.TestCase):
    def test_keeps_newest_pr_directories(self) -> None:
        site = _make_site("pr-1", "pr-2", "pr-10", "branch-main")
        try:
            pages_retention.main([str(site), "2", "pr-10"])
            self.assertFalse((site / "pr-1").exists())
            self.assertTrue((site / "pr-2").exists())
            self.assertTrue((site / "pr-10").exists())
            self.assertTrue((site / "branch-main").exists())
        finally:
            shutil.rmtree(site)

    def test_current_deployment_survives_even_when_old(self) -> None:
        site = _make_site("pr-5", "pr-90", "pr-100")
        try:
            pages_retention.main([str(site), "1", "pr-5"])
            self.assertTrue((site / "pr-5").exists())
            self.assertTrue((site / "pr-100").exists())
            self.assertFalse((site / "pr-90").exists())
        finally:
            shutil.rmtree(site)

    def test_ignores_non_pr_entries(self) -> None:
        site = _make_site("pr-1", "pr-2", "branch-main", "pr-x")
        try:
            _ = (site / ".nojekyll").write_text("", encoding="utf-8")
            pages_retention.main([str(site), "1", "pr-2"])
            self.assertFalse((site / "pr-1").exists())
            self.assertTrue((site / "pr-2").exists())
            self.assertTrue((site / "branch-main").exists())
            self.assertTrue((site / "pr-x").exists())
            self.assertTrue((site / ".nojekyll").exists())
        finally:
            shutil.rmtree(site)

    def test_rejects_invalid_keep_count(self) -> None:
        site = _make_site("pr-1")
        try:
            for invalid in ("0", "-1", "two", ""):
                with self.assertRaises(SystemExit):
                    pages_retention.main([str(site), invalid, "pr-1"])
            self.assertTrue((site / "pr-1").exists())
        finally:
            shutil.rmtree(site)

    def test_rejects_missing_site_root(self) -> None:
        with self.assertRaises(SystemExit):
            pages_retention.main(["/nonexistent/badgers-site", "1", "pr-1"])

    def test_rejects_wrong_argument_count(self) -> None:
        with self.assertRaises(SystemExit):
            pages_retention.main([str(Path(tempfile.gettempdir())), "1"])


if __name__ == "__main__":
    unittest.main()
