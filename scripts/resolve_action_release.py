#!/usr/bin/env python3
"""Resolve a compatible prebuilt CLI release for the composite Action."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


REPOSITORY = "devgony/badgers"
FULL_VERSION = re.compile(
    r"^v\d+\.\d+\.\d+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?(?:\+[0-9A-Za-z][0-9A-Za-z.-]*)?$"
)
STABLE_VERSION = re.compile(
    r"^v(\d+)\.(\d+)\.(\d+)(?:\+[0-9A-Za-z][0-9A-Za-z.-]*)?$"
)
MAJOR_VERSION = re.compile(r"^v\d+$")
TARGETS = {
    ("Linux", "X64"): "x86_64-unknown-linux-gnu",
    ("Linux", "ARM64"): "aarch64-unknown-linux-gnu",
    ("macOS", "X64"): "x86_64-apple-darwin",
    ("macOS", "ARM64"): "aarch64-apple-darwin",
}


def target_for(runner_os: str, runner_arch: str) -> str | None:
    return TARGETS.get((runner_os, runner_arch))


def release_selector(cli_version: str, action_ref: str) -> tuple[str, str] | None:
    if cli_version == "source":
        return None
    if cli_version == "auto":
        if FULL_VERSION.fullmatch(action_ref):
            return ("exact", action_ref)
        if MAJOR_VERSION.fullmatch(action_ref):
            return ("major", action_ref)
        return None
    if FULL_VERSION.fullmatch(cli_version):
        return ("exact", cli_version)
    raise ValueError("cli-version must be 'auto', 'source', or an exact vX.Y.Z tag")


def has_target_assets(release: dict[str, Any], target: str) -> bool:
    names = {
        asset.get("name")
        for asset in release.get("assets", [])
        if isinstance(asset, dict)
    }
    archive = f"badgers-{target}.tar.gz"
    return archive in names and f"{archive}.sha256" in names


def select_release(
    releases: list[dict[str, Any]], mode: str, requested: str, target: str
) -> str | None:
    candidates: list[tuple[tuple[int, int, int], str]] = []
    for release in releases:
        tag = release.get("tag_name")
        if release.get("draft") or not isinstance(tag, str):
            continue
        if mode == "exact":
            if tag == requested and has_target_assets(release, target):
                return tag
            continue

        match = STABLE_VERSION.fullmatch(tag)
        if (
            release.get("prerelease")
            or match is None
            or not tag.startswith(f"{requested}.")
            or not has_target_assets(release, target)
        ):
            continue
        candidates.append((tuple(int(part) for part in match.groups()), tag))
    return max(candidates)[1] if candidates else None


def fetch_releases(mode: str, requested: str) -> list[dict[str, Any]]:
    base = f"https://api.github.com/repos/{REPOSITORY}/releases"
    if mode == "exact":
        url = f"{base}/tags/{urllib.parse.quote(requested, safe='')}"
    else:
        url = f"{base}?per_page=100"

    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "badgers-action",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token := os.environ.get("GITHUB_TOKEN"):
        headers["Authorization"] = f"Bearer {token}"

    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=15) as response:
        payload = json.load(response)
    if mode == "exact":
        return [payload] if isinstance(payload, dict) else []
    return payload if isinstance(payload, list) else []


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli-version", required=True)
    parser.add_argument("--action-ref", required=True)
    parser.add_argument("--runner-os", required=True)
    parser.add_argument("--runner-arch", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    target = target_for(args.runner_os, args.runner_arch)
    if target is None:
        return 0

    try:
        selector = release_selector(args.cli_version, args.action_ref)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if selector is None:
        return 0

    mode, requested = selector
    try:
        releases = fetch_releases(mode, requested)
    except (OSError, urllib.error.URLError, json.JSONDecodeError) as error:
        print(
            f"warning: could not query badgers releases ({error}); using a source build",
            file=sys.stderr,
        )
        return 0

    if version := select_release(releases, mode, requested, target):
        print(version)
    else:
        print(
            f"warning: no {requested} release contains prebuilt assets for {target}; "
            "using a source build",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
