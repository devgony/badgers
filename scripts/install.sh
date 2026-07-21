#!/bin/sh
set -eu

repository="devgony/badgers"
install_dir="${BADGERS_INSTALL_DIR:-${HOME}/.local/bin}"
version="${BADGERS_VERSION:-latest}"
os="${BADGERS_INSTALLER_OS:-$(uname -s)}"
arch="${BADGERS_INSTALLER_ARCH:-$(uname -m)}"

case "${os}:${arch}" in
  Darwin:arm64 | Darwin:aarch64)
    target="aarch64-apple-darwin"
    ;;
  Darwin:x86_64)
    target="x86_64-apple-darwin"
    ;;
  Linux:x86_64 | Linux:amd64)
    target="x86_64-unknown-linux-gnu"
    ;;
  Linux:arm64 | Linux:aarch64)
    target="aarch64-unknown-linux-gnu"
    ;;
  *)
    echo "error: unsupported platform ${os}/${arch}" >&2
    exit 1
    ;;
esac

if [ "${1:-}" = "--print-target" ]; then
  echo "${target}"
  exit 0
fi
if [ "${#}" -ne 0 ]; then
  echo "usage: install.sh [--print-target]" >&2
  exit 2
fi

asset="badgers-${target}.tar.gz"
case "${version}" in
  latest)
    base_url="https://github.com/${repository}/releases/latest/download"
    ;;
  v[0-9]*.[0-9]*.[0-9]*)
    case "${version}" in
      *[!0-9A-Za-z._+-]*)
        echo "error: invalid BADGERS_VERSION ${version}" >&2
        exit 2
        ;;
    esac
    base_url="https://github.com/${repository}/releases/download/${version}"
    ;;
  *)
    echo "error: BADGERS_VERSION must be latest or an exact vX.Y.Z tag" >&2
    exit 2
    ;;
esac
temporary_dir=$(mktemp -d)
trap 'rm -rf "${temporary_dir}"' EXIT HUP INT TERM

curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  --output "${temporary_dir}/${asset}" "${base_url}/${asset}"
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  --output "${temporary_dir}/${asset}.sha256" "${base_url}/${asset}.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "${temporary_dir}" && sha256sum --check "${asset}.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "${temporary_dir}" && shasum -a 256 --check "${asset}.sha256")
else
  echo "error: sha256sum or shasum is required" >&2
  exit 1
fi

tar -xzf "${temporary_dir}/${asset}" -C "${temporary_dir}"
if [ ! -f "${temporary_dir}/badgers" ]; then
  echo "error: release archive does not contain badgers" >&2
  exit 1
fi

mkdir -p "${install_dir}"
install -m 755 "${temporary_dir}/badgers" "${install_dir}/badgers"
echo "installed: ${install_dir}/badgers"
