#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUNTIME_DIR="${RUNTIME_DIR:-${ROOT_DIR}/runtime}"
CERT_DIR="${CERT_DIR:-${ROOT_DIR}/certs}"
FRONTEND_PORT="${PORT:-8443}"
BACKEND_PORT="${BACKEND_PORT:-9000}"

mkdir -p "${RUNTIME_DIR}"
CERT_DIR="${CERT_DIR}" "${ROOT_DIR}/helpers/shell/generate_cert.sh"

cleanup() {
  kill "${FRONTEND_PID:-}" "${BACKEND_PID:-}" 2>/dev/null || true
  rm -f "${RUNTIME_DIR}/frontend.pid" "${RUNTIME_DIR}/backend.pid"
}
trap cleanup EXIT INT TERM

python3 "${ROOT_DIR}/backend/server.py" --port "${BACKEND_PORT}" \
  >"${RUNTIME_DIR}/backend.log" 2>&1 &
BACKEND_PID=$!
echo "${BACKEND_PID}" >"${RUNTIME_DIR}/backend.pid"

PORT="${FRONTEND_PORT}" \
BACKEND_URL="${BACKEND_URL:-http://127.0.0.1:${BACKEND_PORT}}" \
TLS_CERT="${CERT_DIR}/server.crt" \
TLS_KEY="${CERT_DIR}/server.key" \
node "${ROOT_DIR}/frontend/server.mjs" \
  >"${RUNTIME_DIR}/frontend.log" 2>&1 &
FRONTEND_PID=$!
echo "${FRONTEND_PID}" >"${RUNTIME_DIR}/frontend.pid"

echo "Frontend: https://localhost:${FRONTEND_PORT}"
echo "Backend:  http://127.0.0.1:${BACKEND_PORT}"
wait "${FRONTEND_PID}"
