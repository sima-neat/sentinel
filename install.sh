#!/usr/bin/env bash
set -euo pipefail

INSTALL_ROOT="${SIMA_SENTINEL_INSTALL_ROOT:-/opt/simaai/sentinel}"
SERVICE_NAME="simaai-sentinel.service"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_SRC="${SCRIPT_DIR}/simaai-sentinel"
SERVICE_SRC="${SCRIPT_DIR}/${SERVICE_NAME}"

if [[ "$(uname -m)" != "aarch64" ]]; then
  echo "Warning: Sentinel is intended for Modalix DevKit aarch64 targets." >&2
fi

if [[ ! -f "${BINARY_SRC}" ]]; then
  echo "Required package file not found: ${BINARY_SRC}" >&2
  exit 1
fi

if [[ ! -f "${SERVICE_SRC}" ]]; then
  echo "Required package file not found: ${SERVICE_SRC}" >&2
  exit 1
fi

SUDO=""
if [[ "${EUID}" -ne 0 ]]; then
  if command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
  else
    echo "Root privileges are required to install Sentinel service files." >&2
    exit 1
  fi
fi

echo "Installing SiMa.ai Sentinel to ${INSTALL_ROOT}..."
${SUDO} mkdir -p "${INSTALL_ROOT}"
${SUDO} install -m 0755 "${BINARY_SRC}" "${INSTALL_ROOT}/simaai-sentinel"
${SUDO} install -m 0644 "${SERVICE_SRC}" "${INSTALL_ROOT}/${SERVICE_NAME}"

${SUDO} install -m 0755 -d /usr/local/bin
${SUDO} install -m 0755 "${BINARY_SRC}" /usr/local/bin/simaai-sentinel

${SUDO} install -m 0755 -d /run/simaai-sentinel
${SUDO} install -m 0755 -d /var/log/simaai-sentinel
${SUDO} install -m 0644 "${SERVICE_SRC}" "/etc/systemd/system/${SERVICE_NAME}"

${SUDO} systemctl daemon-reload
${SUDO} systemctl enable "${SERVICE_NAME}"
${SUDO} systemctl restart "${SERVICE_NAME}"

echo "Sentinel installed."
echo "Try: simaai-sentinel status"
echo "Try: simaai-sentinel"
