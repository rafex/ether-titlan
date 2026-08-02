#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE_NAME="${CI_IMAGE_NAME:-qr-light-transfer-ci}"
CONTAINER_ENGINE="${CONTAINER_ENGINE:-podman}"
ARTIFACT_DIR="${ARTIFACT_DIR:-${ROOT_DIR}/ci/artifacts}"

mkdir -p "${ARTIFACT_DIR}"

build_args=(build -t "${IMAGE_NAME}" -f "${ROOT_DIR}/ci/Containerfile" "${ROOT_DIR}")
run_args=(run --rm -v "${ARTIFACT_DIR}:/artifacts:Z" "${IMAGE_NAME}")
if [[ -n "${CONTAINER_CONNECTION:-}" ]]; then
  "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" "${build_args[@]}"
  "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" "${run_args[@]}"
else
  "${CONTAINER_ENGINE}" "${build_args[@]}"
  "${CONTAINER_ENGINE}" "${run_args[@]}"
fi
