from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from .pages_report_equivalent import equivalent


def _index(generated: str, summary: str = "80%") -> str:
    return (
        '<html><header><div class="meta">owner/repo @ abc1234 · generated '
        f'{generated}</div></header><main>{summary}</main></html>'
    )


class PagesReportEquivalentTests(unittest.TestCase):
    def test_rerun_with_new_generation_time_reuses_previous_report(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            new = Path(directory, "new")
            previous = Path(directory, "previous")
            new.mkdir()
            previous.mkdir()
            (new / "index.html").write_text(_index("2026-11-05T10:00:00Z"))
            (previous / "index.html").write_text(_index("2026-11-05T09:00:00Z"))
            (new / "file-0.html").write_text("<html>file coverage</html>")
            (previous / "file-0.html").write_text("<html>file coverage</html>")
            self.assertTrue(equivalent(new, previous))

            (new / "index.html").write_text(_index("2026-11-05T10:00:00Z", "81%"))
            self.assertFalse(equivalent(new, previous))

    def test_changed_or_removed_file_page_is_not_reused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            new = Path(directory, "new")
            previous = Path(directory, "previous")
            new.mkdir()
            previous.mkdir()
            (new / "index.html").write_text(_index("new"))
            (previous / "index.html").write_text(_index("old"))
            (new / "file-0.html").write_text("covered")
            (previous / "file-0.html").write_text("uncovered")
            self.assertFalse(equivalent(new, previous))

            (new / "file-0.html").unlink()
            self.assertFalse(equivalent(new, previous))

    def test_unrecognized_index_change_is_not_discarded(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            new = Path(directory, "new")
            previous = Path(directory, "previous")
            new.mkdir()
            previous.mkdir()
            (new / "index.html").write_text("<html>new template</html>")
            (previous / "index.html").write_text("<html>old template</html>")
            self.assertFalse(equivalent(new, previous))


if __name__ == "__main__":
    unittest.main()
