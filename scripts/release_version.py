#!/usr/bin/env python3
"""Validate and update the synchronized Badgers workspace version."""

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import cast


INTERNAL_PACKAGES = (
    "badge-rs-core",
    "badge-rs-github",
    "badge-rs-storage",
    "badge-rs-lcov",
    "badge-rs",
)
INTERNAL_DEPENDENCIES = INTERNAL_PACKAGES[:-1]
INHERITED_MANIFESTS = (
    "crates/badgers-core/Cargo.toml",
    "crates/badgers-github/Cargo.toml",
    "crates/badgers-storage/Cargo.toml",
    "crates/badgers-lcov/Cargo.toml",
)
CLI_MANIFEST = "crates/badgers-cli/Cargo.toml"
STABLE_VERSION = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


class VersionError(ValueError):
    """Raised when release version sources are invalid or inconsistent."""


def _load_toml(path: Path) -> dict[str, object]:
    try:
        with path.open("rb") as source:
            return tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise VersionError(f"could not read {path}: {error}") from error


def _stable_parts(version: object, source: str) -> tuple[int, int, int]:
    if not isinstance(version, str) or (match := STABLE_VERSION.fullmatch(version)) is None:
        raise VersionError(f"{source} must be a stable semantic version (X.Y.Z)")
    major, minor, patch = match.groups()
    return (int(major), int(minor), int(patch))


def synchronized_version(root: Path) -> str:
    workspace = _load_toml(root / "Cargo.toml")
    workspace_table = workspace.get("workspace")
    if not isinstance(workspace_table, dict):
        raise VersionError("Cargo.toml is missing workspace version metadata")
    workspace_table = cast(dict[str, object], workspace_table)
    package_table = workspace_table.get("package")
    dependencies = workspace_table.get("dependencies")
    if not isinstance(package_table, dict) or not isinstance(dependencies, dict):
        raise VersionError("Cargo.toml is missing workspace version metadata")
    package_table = cast(dict[str, object], package_table)
    dependencies = cast(dict[str, object], dependencies)
    version = package_table.get("version")
    _ = _stable_parts(version, "workspace.package.version")
    assert isinstance(version, str)

    for name in INTERNAL_DEPENDENCIES:
        dependency = dependencies.get(name)
        if not isinstance(dependency, dict) or "version" not in dependency:
            raise VersionError(
                f"Cargo.toml is missing the {name} workspace dependency version"
            )
        dependency_version = cast(dict[str, object], dependency)["version"]
        if dependency_version != version:
            raise VersionError(
                f"{name} workspace dependency is {dependency_version!r}, expected {version}"
            )

    for relative_path in INHERITED_MANIFESTS:
        manifest = _load_toml(root / relative_path)
        manifest_package = manifest.get("package")
        if not isinstance(manifest_package, dict):
            raise VersionError(f"{relative_path} must inherit the workspace version")
        manifest_package = cast(dict[str, object], manifest_package)
        manifest_version = manifest_package.get("version")
        if not isinstance(manifest_version, dict):
            raise VersionError(f"{relative_path} must inherit the workspace version")
        manifest_version = cast(dict[str, object], manifest_version)
        inherited = manifest_version.get("workspace")
        if inherited is not True:
            raise VersionError(f"{relative_path} must inherit the workspace version")

    cli = _load_toml(root / CLI_MANIFEST)
    cli_package = cli.get("package")
    if not isinstance(cli_package, dict) or "version" not in cli_package:
        raise VersionError(f"{CLI_MANIFEST} is missing package.version")
    cli_version = cast(dict[str, object], cli_package)["version"]
    if cli_version != version:
        raise VersionError(f"CLI version is {cli_version!r}, expected {version}")

    lock = _load_toml(root / "Cargo.lock")
    packages = lock.get("package")
    if not isinstance(packages, list):
        raise VersionError("Cargo.lock is missing package entries")
    package_tables: list[dict[str, object]] = []
    for package in cast(list[object], packages):
        if isinstance(package, dict):
            package_tables.append(cast(dict[str, object], package))
    for name in INTERNAL_PACKAGES:
        matches = [package for package in package_tables if package.get("name") == name]
        if len(matches) != 1:
            raise VersionError(f"Cargo.lock must contain exactly one {name} package")
        lock_version = matches[0].get("version")
        if lock_version != version:
            raise VersionError(
                f"Cargo.lock {name} version is {lock_version!r}, expected {version}"
            )
    return version


def next_version(version: str, bump: str) -> str:
    major, minor, patch = _stable_parts(version, "current version")
    if bump == "major":
        return f"{major + 1}.0.0"
    if bump == "minor":
        return f"{major}.{minor + 1}.0"
    if bump == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise VersionError("bump must be exactly major, minor, or patch")


def _replace_section_value(
    content: str, section: str, key: str, old: str, new: str
) -> str:
    pattern = re.compile(
        rf"(?ms)(^\[{re.escape(section)}\]\s*.*?^\s*{re.escape(key)}\s*=\s*\")"
        + rf"{re.escape(old)}(\")"
    )
    updated, count = pattern.subn(rf"\g<1>{new}\g<2>", content, count=1)
    if count != 1:
        raise VersionError(f"could not update {section}.{key}")
    return updated


def _updated_workspace(content: str, old: str, new: str) -> str:
    updated = _replace_section_value(content, "workspace.package", "version", old, new)
    for name in INTERNAL_DEPENDENCIES:
        pattern = re.compile(
            rf'(?m)^(\s*{re.escape(name)}\s*=\s*\{{[^\n}}]*\bversion\s*=\s*")'
            + rf"{re.escape(old)}(\"[^\n}}]*\}}\s*)$"
        )
        updated, count = pattern.subn(rf"\g<1>{new}\g<2>", updated, count=1)
        if count != 1:
            raise VersionError(f"could not update {name} workspace dependency")
    return updated


def _updated_lock(content: str, old: str, new: str) -> str:
    blocks = re.split(r"(?=^\[\[package\]\])", content, flags=re.MULTILINE)
    updated_names: set[str] = set()
    for index, block in enumerate(blocks):
        name_match = re.search(r'(?m)^name\s*=\s*"([^"]+)"$', block)
        if name_match is None or name_match.group(1) not in INTERNAL_PACKAGES:
            continue
        name = name_match.group(1)
        blocks[index], count = re.subn(
            rf'(?m)^(version\s*=\s*"){re.escape(old)}("\s*)$',
            rf"\g<1>{new}\g<2>",
            block,
            count=1,
        )
        if count != 1 or name in updated_names:
            raise VersionError(f"could not uniquely update Cargo.lock package {name}")
        updated_names.add(name)
    package_names = {str(name) for name in INTERNAL_PACKAGES}
    missing = package_names.difference(updated_names)
    if missing:
        raise VersionError(f"Cargo.lock is missing packages: {', '.join(sorted(missing))}")
    return "".join(blocks)


def _write_atomic(path: Path, content: str) -> None:
    descriptor, temporary_name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as destination:
            _ = destination.write(content)
        os.replace(temporary_name, path)
    except BaseException:
        try:
            os.unlink(temporary_name)
        except FileNotFoundError:
            pass
        raise


def bump_workspace(root: Path, bump: str) -> str:
    old = synchronized_version(root)
    new = next_version(old, bump)
    paths = (root / "Cargo.toml", root / CLI_MANIFEST, root / "Cargo.lock")
    contents = [path.read_text(encoding="utf-8") for path in paths]
    updated = [
        _updated_workspace(contents[0], old, new),
        _replace_section_value(contents[1], "package", "version", old, new),
        _updated_lock(contents[2], old, new),
    ]
    for content in updated:
        try:
            _ = tomllib.loads(content)
        except tomllib.TOMLDecodeError as error:
            raise VersionError(f"version update produced invalid TOML: {error}") from error
    for path, content in zip(paths, updated, strict=True):
        _write_atomic(path, content)
    if synchronized_version(root) != new:
        raise VersionError("updated version sources did not remain synchronized")
    return new


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parents[1]
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    _ = subparsers.add_parser("current")
    next_parser = subparsers.add_parser("next")
    _ = next_parser.add_argument("bump")
    bump_parser = subparsers.add_parser("bump")
    _ = bump_parser.add_argument("bump")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    root = cast(Path, args.root)
    command = cast(str, args.command)
    try:
        current = synchronized_version(root)
        if command == "current":
            result = current
        elif command == "next":
            result = next_version(current, cast(str, args.bump))
        else:
            result = bump_workspace(root, cast(str, args.bump))
    except VersionError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
