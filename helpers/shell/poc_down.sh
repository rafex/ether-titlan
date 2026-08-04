#!/usr/bin/env bash
set -euo pipefail

CONTAINER_NAME="${CONTAINER_NAME:-tona-transfer-poc}"
CONTAINER_ENGINE="${CONTAINER_ENGINE:-podman}"

if [[ -n "${CONTAINER_CONNECTION:-}" ]]; then
  "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" rm -f "${CONTAINER_NAME}"
else
  "${CONTAINER_ENGINE}" rm -f "${CONTAINER_NAME}"
fi
