from __future__ import annotations

import hashlib
import os
import subprocess
import tempfile
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

    def test_rejects_invalid_version(self) -> None:
        environment = os.environ.copy()
        environment["BADGERS_INSTALLER_OS"] = "Linux"
        environment["BADGERS_INSTALLER_ARCH"] = "x86_64"
        environment["BADGERS_VERSION"] = "1.2.3"
        result = subprocess.run(
            ["sh", str(INSTALLER)],
            capture_output=True,
            text=True,
            env=environment,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("BADGERS_VERSION must be latest or an exact", result.stderr)

    def test_exact_version_downloads_and_installs_matching_asset(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            asset = "badgers-x86_64-unknown-linux-gnu.tar.gz"
            payload = root / "payload"
            payload.mkdir()
            (payload / "badgers").write_text("prebuilt binary\n")
            archive = root / asset
            subprocess.run(
                ["tar", "-C", str(payload), "-czf", str(archive), "badgers"],
                check=True,
            )
            checksum = root / f"{asset}.sha256"
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            checksum.write_text(f"{digest}  {asset}\n")

            fake_bin = root / "bin"
            fake_bin.mkdir()
            fake_curl = fake_bin / "curl"
            fake_curl.write_text(
                """#!/bin/sh
set -eu
output=
url=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) output="$2"; shift 2 ;;
    http*) url="$1"; shift ;;
    *) shift ;;
  esac
done
printf '%s\n' "$url" >> "$BADGERS_CURL_LOG"
case "$url" in
  *.sha256) cp "$BADGERS_FIXTURE_DIR/$BADGERS_ASSET.sha256" "$output" ;;
  *) cp "$BADGERS_FIXTURE_DIR/$BADGERS_ASSET" "$output" ;;
esac
"""
            )
            fake_curl.chmod(0o755)

            install_dir = root / "install"
            curl_log = root / "curl.log"
            environment = os.environ.copy()
            environment.update(
                {
                    "BADGERS_ASSET": asset,
                    "BADGERS_CURL_LOG": str(curl_log),
                    "BADGERS_FIXTURE_DIR": str(root),
                    "BADGERS_INSTALL_DIR": str(install_dir),
                    "BADGERS_INSTALLER_ARCH": "x86_64",
                    "BADGERS_INSTALLER_OS": "Linux",
                    "BADGERS_VERSION": "v1.2.3",
                    "PATH": f"{fake_bin}{os.pathsep}{environment['PATH']}",
                }
            )
            subprocess.run(["sh", str(INSTALLER)], check=True, env=environment)

            self.assertEqual(
                (install_dir / "badgers").read_text(), "prebuilt binary\n"
            )
            base = "https://github.com/devgony/badgers/releases/download/v1.2.3"
            self.assertEqual(
                curl_log.read_text().splitlines(),
                [f"{base}/{asset}", f"{base}/{asset}.sha256"],
            )


if __name__ == "__main__":
    unittest.main()
