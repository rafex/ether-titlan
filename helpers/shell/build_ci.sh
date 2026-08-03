#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE_NAME="${CI_IMAGE_NAME:-tona-transfer-ci}"
CONTAINER_ENGINE="${CONTAINER_ENGINE:-podman}"
ARTIFACT_DIR="${ARTIFACT_DIR:-${ROOT_DIR}/ci/artifacts}"

mkdir -p "${ARTIFACT_DIR}"

build_args=(build -t "${IMAGE_NAME}" -f "${ROOT_DIR}/ci/Containerfile" "${ROOT_DIR}")
if [[ -n "${CONTAINER_CONNECTION:-}" ]]; then
  "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" "${build_args[@]}"
  # A remote Podman service cannot mount a host path from this machine.
  # Run the CI container with an internal artifact directory and copy the
  # result back through the Podman API after the container exits.
  run_name="${IMAGE_NAME}-run-$$"
  cleanup_remote() {
    "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" rm -f "${run_name}" >/dev/null 2>&1 || true
  }
  trap cleanup_remote EXIT
  "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" run --name "${run_name}" "${IMAGE_NAME}"
  "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" cp "${run_name}:/artifacts/." "${ARTIFACT_DIR}/"
  cleanup_remote
  trap - EXIT
else
  run_args=(run --rm -v "${ARTIFACT_DIR}:/artifacts:Z" "${IMAGE_NAME}")
  "${CONTAINER_ENGINE}" "${build_args[@]}"
  "${CONTAINER_ENGINE}" "${run_args[@]}"
fi
