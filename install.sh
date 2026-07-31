#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="simaai-sentinel.service"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_SRC="${SCRIPT_DIR}/simaai-sentinel"
SERVICE_SRC="${SCRIPT_DIR}/${SERVICE_NAME}"
PLAYBOOK_REF="__SENTINEL_PLAYBOOK_REF__"
PLAYBOOK_SOURCE="gh:sima-neat/sentinel/skills/use-sentinel@${PLAYBOOK_REF}"

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

echo "Installing SiMa.ai Sentinel system service..."
${SUDO} install -m 0755 -d /usr/local/bin
${SUDO} install -m 0755 "${BINARY_SRC}" /usr/local/bin/simaai-sentinel

${SUDO} install -m 0755 -d /run/simaai-sentinel
${SUDO} install -m 0755 -d /var/log/simaai-sentinel
${SUDO} install -m 0775 -d /var/lib/simaai-sentinel /var/lib/simaai-sentinel/runs
DATA_GROUP="${SIMA_SENTINEL_DATA_GROUP:-sima}"
if ! getent group "${DATA_GROUP}" >/dev/null 2>&1; then
  DATA_GROUP="${SUDO_USER:-root}"
fi
${SUDO} chown "root:${DATA_GROUP}" /var/lib/simaai-sentinel /var/lib/simaai-sentinel/runs
${SUDO} install -m 0644 "${SERVICE_SRC}" "/etc/systemd/system/${SERVICE_NAME}"

${SUDO} systemctl daemon-reload
${SUDO} systemctl enable "${SERVICE_NAME}"
${SUDO} systemctl restart "${SERVICE_NAME}"

# Keep agent-directory ownership and playbook registry management in sima-cli.
# This command intentionally runs as the invoking user, not through sudo.
if [[ "${PLAYBOOK_REF}" =~ ^[0-9a-f]{40}$ ]]; then
  SIMA_CLI_BIN="${SIMA_CLI:-}"
  if [[ -z "${SIMA_CLI_BIN}" ]] && command -v sima-cli >/dev/null 2>&1; then
    SIMA_CLI_BIN="$(command -v sima-cli)"
  fi
  if [[ -z "${SIMA_CLI_BIN}" && -x "${HOME}/.sima-cli/.venv/bin/sima-cli" ]]; then
    SIMA_CLI_BIN="${HOME}/.sima-cli/.venv/bin/sima-cli"
  fi

  if [[ -n "${SIMA_CLI_BIN}" ]]; then
    echo "Installing Sentinel agent skill through sima-cli playbooks..."
    if ! SIMA_CLI_CHECK_FOR_UPDATE=0 "${SIMA_CLI_BIN}" playbooks install --force "${PLAYBOOK_SOURCE}"; then
      # sima-cli <= 2.1.15 treats commit SHAs as branch names when git is not
      # available and does not preserve the archive extension. Keep the
      # installation managed by sima-cli, but materialize the immutable GitHub
      # archive locally for compatibility with those releases.
      if ! command -v curl >/dev/null 2>&1; then
        echo "Error: curl is required to install the Sentinel skill with this sima-cli version." >&2
        exit 1
      fi
      echo "Retrying Sentinel agent skill installation from its commit archive..."
      PLAYBOOK_TMP_DIR="$(mktemp -d)"
      trap 'rm -rf -- "${PLAYBOOK_TMP_DIR}"' EXIT
      PLAYBOOK_ARCHIVE="${PLAYBOOK_TMP_DIR}/sentinel-${PLAYBOOK_REF}.tar.gz"
      curl -fsSL \
        "https://codeload.github.com/sima-neat/sentinel/tar.gz/${PLAYBOOK_REF}" \
        -o "${PLAYBOOK_ARCHIVE}"
      SIMA_CLI_CHECK_FOR_UPDATE=0 "${SIMA_CLI_BIN}" playbooks install --force "${PLAYBOOK_ARCHIVE}"
      rm -rf -- "${PLAYBOOK_TMP_DIR}"
      trap - EXIT
    fi
  else
    echo "Warning: sima-cli was not found; Sentinel is installed, but its agent skill was not registered." >&2
    echo "Install it later with: sima-cli playbooks install ${PLAYBOOK_SOURCE}" >&2
  fi
else
  echo "Warning: package has no valid Git commit for the Sentinel agent skill; skipping playbook installation." >&2
fi

echo "Sentinel installed."
echo "Try: simaai-sentinel status"
echo "Try: simaai-sentinel"
