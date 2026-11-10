#!/usr/bin/env python3
"""Compare Pages bundles while ignoring only the generated time in index.html."""

from __future__ import annotations

import sys
from pathlib import Path

_GENERATED_MARKER = b" \xc2\xb7 generated "
_HEADER_END = b"</div></header>"


def _without_generated_time(html: bytes) -> bytes:
    before, marker, after = html.partition(_GENERATED_MARKER)
    if not marker or _GENERATED_MARKER in after:
        return html
    _, end, rest = after.partition(_HEADER_END)
    if not end:
        return html
    return before + marker + end + rest


def _same_file(left: Path, right: Path) -> bool:
    with left.open("rb") as a, right.open("rb") as b:
        while True:
            chunk = a.read(64 * 1024)
            if chunk != b.read(64 * 1024):
                return False
            if not chunk:
                return True


def equivalent(source: Path, previous: Path) -> bool:
    if not source.is_dir() or not previous.is_dir():
        return False
    source_entries = {path.relative_to(source): path for path in source.rglob("*")}
    previous_entries = {path.relative_to(previous): path for path in previous.rglob("*")}
    if source_entries.keys() != previous_entries.keys() or Path("index.html") not in source_entries:
        return False
    for relative, current in source_entries.items():
        old = previous_entries[relative]
        if current.is_symlink() or old.is_symlink():
            return False
        if current.is_dir() or old.is_dir():
            if not current.is_dir() or not old.is_dir():
                return False
        elif not current.is_file() or not old.is_file():
            return False
        elif relative == Path("index.html"):
            if _without_generated_time(current.read_bytes()) != _without_generated_time(old.read_bytes()):
                return False
        elif not _same_file(current, old):
            return False
    return True


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: pages_report_equivalent.py <new-report> <stored-report>")
    raise SystemExit(0 if equivalent(Path(sys.argv[1]), Path(sys.argv[2])) else 1)
