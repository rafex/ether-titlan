#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
"${ROOT_DIR}/helpers/shell/stop_stack.sh" || true
rm -rf "${ROOT_DIR}/pkg" "${ROOT_DIR}/target" \
  "${ROOT_DIR}/frontend/pkg" "${ROOT_DIR}/frontend/wasm/target"
rm -f "${ROOT_DIR}/runtime"/*.log "${ROOT_DIR}/runtime"/*.pid
echo "Generated frontend and runtime artifacts removed."
