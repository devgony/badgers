from __future__ import annotations

import unittest

from . import resolve_action_release as resolver


def release(
    tag: str,
    target: str,
    *,
    draft: bool = False,
    prerelease: bool = False,
    checksum: bool = True,
) -> dict[str, object]:
    archive = f"badgers-{target}.tar.gz"
    assets = [{"name": archive}]
    if checksum:
        assets.append({"name": f"{archive}.sha256"})
    return {
        "tag_name": tag,
        "draft": draft,
        "prerelease": prerelease,
        "assets": assets,
    }


class ResolveActionReleaseTests(unittest.TestCase):
    def test_supported_runner_targets(self) -> None:
        self.assertEqual(
            resolver.target_for("Linux", "X64"), "x86_64-unknown-linux-gnu"
        )
        self.assertEqual(
            resolver.target_for("Linux", "ARM64"), "aarch64-unknown-linux-gnu"
        )
        self.assertEqual(
            resolver.target_for("macOS", "X64"), "x86_64-apple-darwin"
        )
        self.assertEqual(
            resolver.target_for("macOS", "ARM64"), "aarch64-apple-darwin"
        )
        self.assertIsNone(resolver.target_for("Windows", "X64"))

    def test_auto_uses_versioned_action_refs(self) -> None:
        self.assertEqual(
            resolver.release_selector("auto", "v1.2.3"), ("exact", "v1.2.3")
        )
        self.assertEqual(
            resolver.release_selector("auto", "v1"), ("major", "v1")
        )
        self.assertIsNone(resolver.release_selector("auto", "main"))
        self.assertIsNone(resolver.release_selector("auto", "0123456789abcdef"))

    def test_source_and_exact_overrides(self) -> None:
        self.assertIsNone(resolver.release_selector("source", "v1"))
        self.assertEqual(
            resolver.release_selector("v2.3.4-rc.1", "main"),
            ("exact", "v2.3.4-rc.1"),
        )
        with self.assertRaisesRegex(ValueError, "cli-version"):
            resolver.release_selector("latest", "v1")

    def test_major_selects_highest_stable_release_with_both_assets(self) -> None:
        target = "x86_64-unknown-linux-gnu"
        releases = [
            release("v2.0.0", target),
            release("v1.4.0", target, prerelease=True),
            release("v1.3.0", target, checksum=False),
            release("v1.1.0", target),
            release("v1.2.0", target),
        ]
        self.assertEqual(
            resolver.select_release(releases, "major", "v1", target), "v1.2.0"
        )

    def test_exact_release_requires_matching_assets(self) -> None:
        target = "aarch64-apple-darwin"
        releases = [
            release("v1.2.3", target, checksum=False),
            release("v1.2.2", target),
        ]
        self.assertIsNone(
            resolver.select_release(releases, "exact", "v1.2.3", target)
        )
        self.assertEqual(
            resolver.select_release(releases, "exact", "v1.2.2", target),
            "v1.2.2",
        )


if __name__ == "__main__":
    unittest.main()
