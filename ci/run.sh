#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="/workspace"
ARTIFACT_DIR="${ARTIFACT_DIR:-/artifacts}"

mkdir -p "${ARTIFACT_DIR}/wasm" "${ARTIFACT_DIR}/frontend" "${ARTIFACT_DIR}/reports"

cargo fmt --manifest-path "${ROOT_DIR}/frontend/wasm/Cargo.toml" --check
cargo test --manifest-path "${ROOT_DIR}/frontend/wasm/Cargo.toml"
cargo clippy --manifest-path "${ROOT_DIR}/frontend/wasm/Cargo.toml" --all-targets -- -D warnings
"${ROOT_DIR}/helpers/shell/build_wasm.sh"
(cd "${ROOT_DIR}/frontend" && node --check main.js && node --check packet-worker.js && node --check server.mjs)
python3 "${ROOT_DIR}/helpers/python/check_backend.py"

cp -a "${ROOT_DIR}/frontend/pkg/." "${ARTIFACT_DIR}/wasm/"
cp "${ROOT_DIR}/frontend/index.html" "${ROOT_DIR}/frontend/main.js" \
  "${ROOT_DIR}/frontend/server.mjs" "${ROOT_DIR}/frontend/package.json" \
  "${ARTIFACT_DIR}/frontend/"
cp -a "${ROOT_DIR}/frontend/vendor" "${ARTIFACT_DIR}/frontend/"
cp "${ROOT_DIR}/frontend/packet-worker.js" "${ARTIFACT_DIR}/frontend/"

cat >"${ARTIFACT_DIR}/reports/manifest.txt" <<EOF
tona-transfer-local-ci
rust=$(rustc --version)
node=$(node --version)
python=$(python3 --version)
wasm=frontend/pkg
EOF

echo "Local CI passed; artifacts written to ${ARTIFACT_DIR}."
