#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="${CONTAINER_NAME:-qr-light-transfer-poc}"
CONTAINER_ENGINE="${CONTAINER_ENGINE:-podman}"

if [[ -n "${CONTAINER_CONNECTION:-}" ]]; then
  exec "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" logs -f "${CONTAINER_NAME}"
else
  exec "${CONTAINER_ENGINE}" logs -f "${CONTAINER_NAME}"
fi
