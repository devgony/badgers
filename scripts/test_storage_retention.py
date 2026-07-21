from __future__ import annotations

import json
import os
import shutil
import tempfile
import unittest
from pathlib import Path

from . import storage_retention


def _make_store(prefix: str = "badgers") -> tuple[Path, Path]:
    store = Path(tempfile.mkdtemp())
    repo_dir = store / prefix / "repos" / "owner" / "repo"
    (repo_dir / "commits").mkdir(parents=True)
    (repo_dir / "refs").mkdir()
    (repo_dir / "prs").mkdir()
    return store, repo_dir


def _make_pointer(repo_dir: Path, scope: str, name: str, sha: str) -> None:
    pointer_dir = repo_dir / scope / name
    pointer_dir.mkdir(parents=True, exist_ok=True)
    _ = (pointer_dir / "latest.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "branch": name,
                "commit_sha": sha,
                "committed_at": "2026-07-19T00:00:00Z",
                "snapshot_key": f"badgers/repos/owner/repo/commits/{sha}/coverage.json.zst",
                "updated_at": "2026-07-19T00:01:00Z",
            }
        ),
        encoding="utf-8",
    )


def _make_commit(repo_dir: Path, sha: str) -> Path:
    commit_dir = repo_dir / "commits" / sha
    commit_dir.mkdir(parents=True, exist_ok=True)
    _ = (commit_dir / "coverage.json.zst").write_bytes(b"data")
    return commit_dir


class StorageRetentionTests(unittest.TestCase):
    def test_keeps_referenced_commits(self) -> None:
        store, repo_dir = _make_store()
        try:
            _make_pointer(repo_dir, "refs", "main", "abc123")
            _ = _make_commit(repo_dir, "abc123")
            storage_retention.main([str(store), "badgers"])
            self.assertTrue((repo_dir / "commits" / "abc123").exists())
        finally:
            shutil.rmtree(store)

    def test_prunes_unreferenced_commits(self) -> None:
        store, repo_dir = _make_store()
        try:
            _make_pointer(repo_dir, "refs", "main", "abc123")
            _ = _make_commit(repo_dir, "abc123")
            _ = _make_commit(repo_dir, "deadbeef")
            storage_retention.main([str(store), "badgers"])
            self.assertTrue((repo_dir / "commits" / "abc123").exists())
            self.assertFalse((repo_dir / "commits" / "deadbeef").exists())
        finally:
            shutil.rmtree(store)

    def test_multiple_pointers_union_referenced_shas(self) -> None:
        store, repo_dir = _make_store()
        try:
            _make_pointer(repo_dir, "refs", "main", "sha1")
            _make_pointer(repo_dir, "prs", "42", "sha2")
            _ = _make_commit(repo_dir, "sha1")
            _ = _make_commit(repo_dir, "sha2")
            _ = _make_commit(repo_dir, "sha3")
            storage_retention.main([str(store), "badgers"])
            self.assertTrue((repo_dir / "commits" / "sha1").exists())
            self.assertTrue((repo_dir / "commits" / "sha2").exists())
            self.assertFalse((repo_dir / "commits" / "sha3").exists())
        finally:
            shutil.rmtree(store)

    def test_malformed_pointer_aborts_without_deleting(self) -> None:
        store, repo_dir = _make_store()
        try:
            bad_dir = repo_dir / "refs" / "bad"
            bad_dir.mkdir(parents=True)
            _ = (bad_dir / "latest.json").write_text("{not valid json", encoding="utf-8")
            _ = _make_commit(repo_dir, "orphan123")
            with self.assertRaises(SystemExit):
                storage_retention.main([str(store), "badgers"])
            self.assertTrue((repo_dir / "commits" / "orphan123").exists())
        finally:
            shutil.rmtree(store)

    def test_nonexistent_store_root_raises(self) -> None:
        with self.assertRaises(SystemExit):
            storage_retention.main(["/definitely/does/not/exist/xyzzy", "badgers"])

    def test_empty_store_no_error(self) -> None:
        store, _repo_dir = _make_store()
        try:
            storage_retention.main([str(store), "badgers"])
        finally:
            shutil.rmtree(store)

    def test_html_prefix_key_extends_referenced_shas(self) -> None:
        store, repo_dir = _make_store()
        try:
            pointer_dir = repo_dir / "refs" / "main"
            pointer_dir.mkdir(parents=True, exist_ok=True)
            _ = (pointer_dir / "latest.json").write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "branch": "main",
                        "commit_sha": "htmlsha",
                        "committed_at": "2026-07-19T00:00:00Z",
                        "snapshot_key": "badgers/repos/owner/repo/commits/htmlsha/coverage.json.zst",
                        "html_prefix": "badgers/repos/owner/repo/commits/htmlsha/html",
                        "updated_at": "2026-07-19T00:01:00Z",
                    }
                ),
                encoding="utf-8",
            )
            _ = _make_commit(repo_dir, "htmlsha")
            _ = _make_commit(repo_dir, "unref")
            storage_retention.main([str(store), "badgers"])
            self.assertTrue((repo_dir / "commits" / "htmlsha").exists())
            self.assertFalse((repo_dir / "commits" / "unref").exists())
        finally:
            shutil.rmtree(store)

    @unittest.skipIf(os.name == "nt", "symlinks require admin on Windows")
    def test_symlinked_commit_dir_is_skipped_not_deleted(self) -> None:
        store, repo_dir = _make_store()
        outside = Path(tempfile.mkdtemp())
        try:
            _ = (outside / "data.txt").write_text("sensitive", encoding="utf-8")
            os.symlink(str(outside), str(repo_dir / "commits" / "sym_sha"))
            storage_retention.main([str(store), "badgers"])
            self.assertTrue(outside.exists())
        finally:
            shutil.rmtree(store, ignore_errors=True)
            shutil.rmtree(outside, ignore_errors=True)


if __name__ == "__main__":
    _ = unittest.main()
