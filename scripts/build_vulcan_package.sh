#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-${ROOT_DIR}/dist/sentinel}"
PACKAGE_VERSION="${SENTINEL_PACKAGE_VERSION:-}"
RUST_TARGET="${SENTINEL_RUST_TARGET:-aarch64-unknown-linux-musl}"

cd "${ROOT_DIR}"

if [[ -z "${PACKAGE_VERSION}" ]]; then
  if [[ -n "${GITHUB_SHA:-}" ]]; then
    PACKAGE_VERSION="0.1.0+${GITHUB_SHA:0:12}"
  else
    PACKAGE_VERSION="0.1.0+$(git rev-parse --short=12 HEAD 2>/dev/null || echo local)"
  fi
fi

rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

if ! rustup target list --installed | grep -Fxq "${RUST_TARGET}"; then
  rustup target add "${RUST_TARGET}"
fi

# Keep the package independent of the Vulcan builder's glibc version so the
# same binary runs on supported eLxr DevKit images.
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER:-rust-lld}"
cargo build --release --locked --target "${RUST_TARGET}"

install -m 0755 "target/${RUST_TARGET}/release/simaai-sentinel" "${OUT_DIR}/simaai-sentinel"
install -m 0755 install.sh "${OUT_DIR}/install.sh"
install -m 0644 packaging/systemd/simaai-sentinel.service "${OUT_DIR}/simaai-sentinel.service"

SIMA_CLI_CHECK_FOR_UPDATE=0 sima-cli packages build "${OUT_DIR}" \
  --name "gh:sima-neat/sentinel" \
  --version "${PACKAGE_VERSION}" \
  --description "SiMa.ai Sentinel board sensor monitoring daemon and CLI for Modalix DevKit." \
  --install-script "install.sh" \
  --board-platform modalix

test -f "${OUT_DIR}/metadata.json"
python3 -m json.tool "${OUT_DIR}/metadata.json" >/dev/null
ls -lh "${OUT_DIR}"
