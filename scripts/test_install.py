from __future__ import annotations

import os
import subprocess
import unittest
from pathlib import Path


INSTALLER = Path(__file__).with_name("install.sh")


class InstallerTests(unittest.TestCase):
    def target(self, operating_system: str, architecture: str) -> str:
        environment = os.environ.copy()
        environment["BADGERS_INSTALLER_OS"] = operating_system
        environment["BADGERS_INSTALLER_ARCH"] = architecture
        result = subprocess.run(
            ["sh", str(INSTALLER), "--print-target"],
            check=True,
            capture_output=True,
            text=True,
            env=environment,
        )
        return result.stdout.strip()

    def test_supported_targets(self) -> None:
        cases = {
            ("Darwin", "arm64"): "aarch64-apple-darwin",
            ("Darwin", "x86_64"): "x86_64-apple-darwin",
            ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
            ("Linux", "aarch64"): "aarch64-unknown-linux-gnu",
        }
        for platform, expected in cases.items():
            with self.subTest(platform=platform):
                self.assertEqual(self.target(*platform), expected)

    def test_rejects_unsupported_platform(self) -> None:
        environment = os.environ.copy()
        environment["BADGERS_INSTALLER_OS"] = "Plan9"
        environment["BADGERS_INSTALLER_ARCH"] = "mips"
        result = subprocess.run(
            ["sh", str(INSTALLER), "--print-target"],
            capture_output=True,
            text=True,
            env=environment,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("unsupported platform Plan9/mips", result.stderr)

    def test_shell_syntax(self) -> None:
        subprocess.run(["sh", "-n", str(INSTALLER)], check=True)


if __name__ == "__main__":
    unittest.main()
