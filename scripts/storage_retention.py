#!/usr/bin/env python3
from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path
from typing import cast


def _resolve_store_root(store_root_str: str) -> Path:
    store_root = Path(store_root_str).resolve()
    if not store_root.is_dir():
        raise SystemExit(f"store root is not a directory: {store_root}")
    return store_root


def _within_root(root: Path, candidate: Path) -> bool:
    try:
        _ = candidate.relative_to(root)
        return True
    except ValueError:
        return False


def _extract_sha_from_key(key: str) -> str | None:
    if not key:
        return None
    parts = key.split("/")
    try:
        idx = parts.index("commits")
        sha = parts[idx + 1] if idx + 1 < len(parts) else ""
        return sha if sha and sha not in (".", "..") else None
    except ValueError:
        return None


def _collect_referenced_shas(
    store_root: Path, prefix: str
) -> tuple[set[str], list[Path]]:
    norm_prefix = prefix.strip("/")
    search_base = store_root / norm_prefix if norm_prefix else store_root

    pointer_files: list[Path] = []
    repos_dir = search_base / "repos"
    if repos_dir.is_dir():
        for owner_dir in sorted(repos_dir.iterdir()):
            if not owner_dir.is_dir() or owner_dir.is_symlink():
                continue
            for repo_dir in sorted(owner_dir.iterdir()):
                if not repo_dir.is_dir() or repo_dir.is_symlink():
                    continue
                for scope in ("prs", "refs"):
                    scope_dir = repo_dir / scope
                    if not scope_dir.is_dir():
                        continue
                    for ref_dir in sorted(scope_dir.iterdir()):
                        if not ref_dir.is_dir() or ref_dir.is_symlink():
                            continue
                        pointer = ref_dir / "latest.json"
                        if pointer.is_file():
                            pointer_files.append(pointer)

    referenced: set[str] = set()
    for pf in pointer_files:
        try:
            loaded: object = json.loads(pf.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise SystemExit(
                f"malformed pointer {pf}: {exc}\nAborting without deleting."
            ) from None
        if not isinstance(loaded, dict):
            raise SystemExit(
                f"malformed pointer {pf}: expected JSON object\nAborting without deleting."
            )
        obj = cast("dict[str, object]", loaded)
        sha = obj.get("commit_sha")
        if isinstance(sha, str) and sha not in ("", ".", ".."):
            referenced.add(sha)
        for field in ("snapshot_key", "comparison_key", "report_key", "html_prefix"):
            val = obj.get(field)
            if isinstance(val, str) and val:
                extracted = _extract_sha_from_key(val)
                if extracted:
                    referenced.add(extracted)

    return referenced, pointer_files


def _find_commit_sha_dirs(store_root: Path, prefix: str) -> list[Path]:
    norm_prefix = prefix.strip("/")
    search_base = store_root / norm_prefix if norm_prefix else store_root

    sha_dirs: list[Path] = []
    repos_dir = search_base / "repos"
    if repos_dir.is_dir():
        for owner_dir in repos_dir.iterdir():
            if not owner_dir.is_dir() or owner_dir.is_symlink():
                continue
            for repo_dir in owner_dir.iterdir():
                if not repo_dir.is_dir() or repo_dir.is_symlink():
                    continue
                commits_dir = repo_dir / "commits"
                if not commits_dir.is_dir() or commits_dir.is_symlink():
                    continue
                for sha_dir in commits_dir.iterdir():
                    if sha_dir.is_dir() and not sha_dir.is_symlink():
                        sha_dirs.append(sha_dir)
    return sha_dirs


def main(argv: list[str] | None = None) -> None:
    if argv is None:
        argv = sys.argv[1:]
    if len(argv) != 2:
        raise SystemExit("usage: storage_retention.py <store_root> <prefix>")

    store_root = _resolve_store_root(argv[0])
    prefix = argv[1]

    if "\\" in prefix or any(ord(c) < 32 for c in prefix):
        raise SystemExit("unsafe storage prefix")
    norm_prefix = prefix.strip("/")
    if norm_prefix:
        if any(p in ("", ".", "..") for p in norm_prefix.split("/")):
            raise SystemExit("unsafe storage prefix")

    referenced_shas, pointer_files = _collect_referenced_shas(store_root, prefix)
    summary = f"Found {len(pointer_files)} pointer(s), {len(referenced_shas)} referenced SHA(s)."
    print(summary)

    sha_dirs = _find_commit_sha_dirs(store_root, prefix)

    pruned = kept = 0
    for sha_dir in sha_dirs:
        resolved = sha_dir.resolve()
        if not _within_root(store_root, resolved):
            raise SystemExit(
                f"path escape detected: {sha_dir} resolves to {resolved} outside store root {store_root}"
            )
        if sha_dir.name not in referenced_shas:
            print(f"Pruning: {sha_dir}")
            shutil.rmtree(sha_dir)
            pruned += 1
        else:
            kept += 1

    print(f"Done: pruned {pruned}, kept {kept}.")


if __name__ == "__main__":
    main()
