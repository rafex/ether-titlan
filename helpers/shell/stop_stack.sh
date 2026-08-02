#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUNTIME_DIR="${RUNTIME_DIR:-${ROOT_DIR}/runtime}"

for pid_file in "${RUNTIME_DIR}/frontend.pid" "${RUNTIME_DIR}/backend.pid"; do
  if [[ -s "${pid_file}" ]]; then
    kill "$(<"${pid_file}")" 2>/dev/null || true
    rm -f "${pid_file}"
  fi
done
