#!/usr/bin/env python3
"""Build safe, deterministic pull-request report navigation URLs."""

from __future__ import annotations

import os
import re
from pathlib import Path
from urllib.parse import quote, urlsplit


REPO_PART = re.compile(r"[A-Za-z0-9_.-]+", re.ASCII)


def _repo_parts(value: str, label: str) -> tuple[str, str]:
    parts = value.split("/")
    if (
        len(parts) != 2
        or any(part in ("", ".", "..") for part in parts)
        or any(REPO_PART.fullmatch(part) is None for part in parts)
    ):
        raise ValueError(f"{label} must be owner/repo")
    return parts[0], parts[1]


def _server_url(value: str) -> str:
    if "\\" in value or any(char.isspace() or ord(char) < 32 for char in value):
        raise ValueError("GITHUB_SERVER_URL must be a safe https origin")
    parsed = urlsplit(value)
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
        or parsed.path not in ("", "/")
    ):
        raise ValueError("GITHUB_SERVER_URL must be a safe https origin")
    return value.rstrip("/")


def _pr_number(value: str) -> str:
    if not value.isascii() or not value.isdigit() or int(value) < 1:
        raise ValueError("pull request number must be a positive ASCII integer")
    return value


def _prefix_parts(value: str) -> list[str]:
    if "\\" in value or any(ord(char) < 32 for char in value):
        raise ValueError("unsafe storage prefix")
    normalized = value.strip("/")
    if not normalized:
        return []
    parts = normalized.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise ValueError("unsafe storage prefix")
    return parts


def build_urls(environ: dict[str, str]) -> tuple[str, str]:
    server = _server_url(environ["GITHUB_SERVER_URL"])
    source = _repo_parts(environ["BADGERS_SOURCE_REPO"], "source repository")
    head = _repo_parts(environ["BADGERS_PR_HEAD_REPO"], "pull request head repository")
    pr = _pr_number(environ["BADGERS_PR_NUMBER"])
    encoded_source = [quote(part, safe="") for part in source]
    files_url = "/".join([server, *encoded_source, "pull", pr, "files"])

    storage_repo = environ.get("BADGERS_STORAGE_REPO", "")
    if not storage_repo:
        return files_url, ""

    storage = _repo_parts(storage_repo, "github-storage-repo")
    branch = environ.get("BADGERS_STORAGE_BRANCH", "")
    if (
        not branch
        or "\\" in branch
        or any(char.isspace() or ord(char) < 32 for char in branch)
    ):
        raise ValueError("github-storage-branch is unsafe")
    prefix = _prefix_parts(environ.get("BADGERS_STORAGE_PREFIX", ""))
    if head != source:
        return files_url, ""
    durable_url = "/".join(
        [
            server,
            *(quote(part, safe="") for part in storage),
            "blob",
            quote(branch, safe=""),
            *(quote(part, safe="") for part in prefix),
            "repos",
            *encoded_source,
            "prs",
            pr,
            "README.md",
        ]
    )
    return files_url, durable_url


def main() -> None:
    try:
        files_url, durable_url = build_urls(dict(os.environ))
    except (KeyError, ValueError) as error:
        raise SystemExit(str(error)) from error
    output = Path(os.environ["GITHUB_OUTPUT"])
    with output.open("a", encoding="utf-8") as stream:
        _ = stream.write(f"files-changed-url={files_url}\n")
        _ = stream.write(f"durable-report-url={durable_url}\n")


if __name__ == "__main__":
    main()
