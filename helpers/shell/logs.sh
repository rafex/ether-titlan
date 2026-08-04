#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUNTIME_DIR="${RUNTIME_DIR:-${ROOT_DIR}/runtime}"
tail -f "${RUNTIME_DIR}/backend.log" "${RUNTIME_DIR}/frontend.log"
