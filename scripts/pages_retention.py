#!/usr/bin/env python3
from __future__ import annotations

import re
import shutil
import sys
from pathlib import Path

PR_DIR = re.compile(r"^pr-([0-9]+)$")


def prune(site_root_str: str, keep_count_str: str, current_dest: str) -> None:
    site_root = Path(site_root_str).resolve()
    if not site_root.is_dir():
        raise SystemExit(f"site root is not a directory: {site_root}")
    if not keep_count_str.isdigit() or int(keep_count_str) < 1:
        raise SystemExit("pages-retention must be 'all' or a positive integer")
    keep_count = int(keep_count_str)

    pr_dirs: list[tuple[int, Path]] = []
    for entry in site_root.iterdir():
        match = PR_DIR.fullmatch(entry.name)
        if match and entry.is_dir() and not entry.is_symlink():
            pr_dirs.append((int(match.group(1)), entry))
    pr_dirs.sort(reverse=True)

    keep = {path.name for _, path in pr_dirs[:keep_count]}
    keep.add(current_dest)

    pruned = kept = 0
    for _, path in pr_dirs:
        if path.name in keep:
            kept += 1
        else:
            print(f"Pruning: {path.name}")
            shutil.rmtree(path)
            pruned += 1
    print(f"Done: pruned {pruned}, kept {kept}.")


def main(argv: list[str] | None = None) -> None:
    if argv is None:
        argv = sys.argv[1:]
    if len(argv) != 3:
        raise SystemExit(
            "usage: pages_retention.py <site_root> <keep_count> <current_dest>"
        )
    prune(argv[0], argv[1], argv[2])


if __name__ == "__main__":
    main()
