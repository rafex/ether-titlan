#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE_NAME="${IMAGE_NAME:-qr-light-transfer}"
CONTAINER_ENGINE="${CONTAINER_ENGINE:-podman}"

if [[ -n "${CONTAINER_CONNECTION:-}" ]]; then
  "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" build -t "${IMAGE_NAME}" -f "${ROOT_DIR}/Containerfile" "${ROOT_DIR}"
else
  "${CONTAINER_ENGINE}" build -t "${IMAGE_NAME}" -f "${ROOT_DIR}/Containerfile" "${ROOT_DIR}"
fi
