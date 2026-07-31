#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-${ROOT_DIR}/dist/sentinel}"
PACKAGE_VERSION="${SENTINEL_PACKAGE_VERSION:-}"
RUST_TARGET="${SENTINEL_RUST_TARGET:-aarch64-unknown-linux-musl}"
PLAYBOOK_REF="${GITHUB_SHA:-$(git rev-parse HEAD)}"
GIT_REF_NAME="${GITHUB_REF_NAME:-$(git symbolic-ref --quiet --short HEAD 2>/dev/null || echo detached)}"
GIT_REF_TYPE="${GITHUB_REF_TYPE:-}"
GIT_SHORT_SHA="${PLAYBOOK_REF:0:12}"
BASE_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"

if [[ -z "${GIT_REF_TYPE}" ]]; then
  EXACT_TAG="$(git describe --tags --exact-match HEAD 2>/dev/null || true)"
  if [[ -n "${EXACT_TAG}" ]]; then
    GIT_REF_TYPE="tag"
    GIT_REF_NAME="${EXACT_TAG}"
  fi
fi

cd "${ROOT_DIR}"

if [[ "${GIT_REF_TYPE}" == "tag" ]]; then
  SENTINEL_BUILD_VERSION="${GIT_REF_NAME#v}"
  PACKAGE_VERSION="${PACKAGE_VERSION:-${SENTINEL_BUILD_VERSION}}"
else
  SENTINEL_BUILD_VERSION="${GIT_REF_NAME}:${GIT_SHORT_SHA}"
  PACKAGE_VERSION="${PACKAGE_VERSION:-${BASE_VERSION}+${GIT_SHORT_SHA}}"
fi
export SENTINEL_BUILD_VERSION

echo "Sentinel binary version: ${SENTINEL_BUILD_VERSION}"
echo "Sentinel package version: ${PACKAGE_VERSION}"

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
sed "s/__SENTINEL_PLAYBOOK_REF__/${PLAYBOOK_REF}/g" \
  "${OUT_DIR}/install.sh" > "${OUT_DIR}/install.sh.generated"
mv "${OUT_DIR}/install.sh.generated" "${OUT_DIR}/install.sh"
chmod 0755 "${OUT_DIR}/install.sh"

SIMA_CLI_CHECK_FOR_UPDATE=0 sima-cli packages build "${OUT_DIR}" \
  --name "gh:sima-neat/sentinel" \
  --version "${PACKAGE_VERSION}" \
  --description "SiMa.ai Sentinel board sensor monitoring daemon and CLI for Modalix DevKit." \
  --install-script "install.sh" \
  --board-platform modalix

test -f "${OUT_DIR}/metadata.json"
python3 -m json.tool "${OUT_DIR}/metadata.json" >/dev/null
ls -lh "${OUT_DIR}"
