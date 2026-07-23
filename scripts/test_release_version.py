from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from . import release_version


ROOT_MANIFEST = """\
[workspace]
members = ["crates/*"]

[workspace.package]
version = "1.2.0"

[workspace.dependencies]
badge-rs-core = { path = "crates/badgers-core", version = "1.2.0" }
badge-rs-github = { path = "crates/badgers-github", version = "1.2.0" }
badge-rs-storage = { path = "crates/badgers-storage", version = "1.2.0" }
badge-rs-lcov = { path = "crates/badgers-lcov", version = "1.2.0" }
"""
INHERITED_MANIFEST = """\
[package]
name = "fixture"
version.workspace = true
"""
CLI_MANIFEST = """\
[package]
name = "badge-rs"
version = "1.2.0"
"""


def _fixture(root: Path) -> None:
    _ = (root / "Cargo.toml").write_text(ROOT_MANIFEST, encoding="utf-8")
    for relative_path in release_version.INHERITED_MANIFESTS:
        path = root / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        _ = path.write_text(INHERITED_MANIFEST, encoding="utf-8")
    cli = root / release_version.CLI_MANIFEST
    cli.parent.mkdir(parents=True, exist_ok=True)
    _ = cli.write_text(CLI_MANIFEST, encoding="utf-8")
    packages = "\n".join(
        f'[[package]]\nname = "{name}"\nversion = "1.2.0"\n'
        for name in release_version.INTERNAL_PACKAGES
    )
    _ = (root / "Cargo.lock").write_text(
        f"version = 4\n\n{packages}", encoding="utf-8"
    )


class ReleaseVersionTests(unittest.TestCase):
    def test_calculates_stable_bumps(self) -> None:
        self.assertEqual(release_version.next_version("1.2.3", "major"), "2.0.0")
        self.assertEqual(release_version.next_version("1.2.3", "minor"), "1.3.0")
        self.assertEqual(release_version.next_version("1.2.3", "patch"), "1.2.4")

    def test_rejects_unknown_bump_and_prerelease(self) -> None:
        with self.assertRaisesRegex(release_version.VersionError, "exactly"):
            _ = release_version.next_version("1.2.3", "1")
        with self.assertRaisesRegex(release_version.VersionError, "stable"):
            _ = release_version.next_version("1.2.3-rc.1", "patch")

    def test_reads_synchronized_version(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _fixture(root)
            self.assertEqual(release_version.synchronized_version(root), "1.2.0")

    def test_rejects_inconsistent_dependency_pin(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _fixture(root)
            manifest = (root / "Cargo.toml").read_text(encoding="utf-8")
            _ = (root / "Cargo.toml").write_text(
                manifest.replace(
                    'badge-rs-core = { path = "crates/badgers-core", version = "1.2.0" }',
                    'badge-rs-core = { path = "crates/badgers-core", version = "1.1.0" }',
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(release_version.VersionError, "badge-rs-core"):
                _ = release_version.synchronized_version(root)

    def test_rejects_inconsistent_cli_and_lock_versions(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _fixture(root)
            cli = root / release_version.CLI_MANIFEST
            _ = cli.write_text(
                CLI_MANIFEST.replace("1.2.0", "1.2.1"), encoding="utf-8"
            )
            with self.assertRaisesRegex(release_version.VersionError, "CLI version"):
                _ = release_version.synchronized_version(root)

            _ = cli.write_text(CLI_MANIFEST, encoding="utf-8")
            lock = (root / "Cargo.lock").read_text(encoding="utf-8")
            _ = (root / "Cargo.lock").write_text(
                lock.replace(
                    'name = "badge-rs-lcov"\nversion = "1.2.0"',
                    'name = "badge-rs-lcov"\nversion = "1.1.0"',
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(release_version.VersionError, "Cargo.lock badge-rs-lcov"):
                _ = release_version.synchronized_version(root)

    def test_bump_updates_only_temporary_version_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _fixture(root)
            self.assertEqual(release_version.bump_workspace(root, "minor"), "1.3.0")
            self.assertEqual(release_version.synchronized_version(root), "1.3.0")
            self.assertNotIn("1.2.0", (root / "Cargo.toml").read_text(encoding="utf-8"))
            self.assertNotIn("1.2.0", (root / "Cargo.lock").read_text(encoding="utf-8"))


if __name__ == "__main__":
    _ = unittest.main()
