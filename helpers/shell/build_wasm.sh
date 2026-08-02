#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WASM_DIR="${ROOT_DIR}/frontend/wasm"

command -v wasm-pack >/dev/null 2>&1 || {
  echo "wasm-pack is required" >&2
  exit 1
}

cd "${WASM_DIR}"
wasm-pack build --target web --release --out-dir ../pkg
