#!/usr/bin/env bash
set -euo pipefail

IMAGE_NAME="${IMAGE_NAME:-tona-transfer}"
CONTAINER_NAME="${CONTAINER_NAME:-tona-transfer-poc}"
CONTAINER_ENGINE="${CONTAINER_ENGINE:-podman}"
POC_BIND_IP="${POC_BIND_IP:-192.168.3.175}"
POC_PORT="${POC_PORT:-30000}"
CERT_ALT_NAME="${CERT_ALT_NAME:-${POC_BIND_IP}}"

run_engine() {
  if [[ -n "${CONTAINER_CONNECTION:-}" ]]; then
    "${CONTAINER_ENGINE}" --connection "${CONTAINER_CONNECTION}" "$@"
  else
    "${CONTAINER_ENGINE}" "$@"
  fi
}

run_engine rm -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
run_engine run -d \
  --name "${CONTAINER_NAME}" \
  --restart unless-stopped \
  --cap-drop=ALL \
  --security-opt=no-new-privileges \
  --memory=512m \
  --pids-limit=128 \
  --health-cmd='node -e "const https=require(\"https\"); const r=https.get(\"https://127.0.0.1:8443/api/health\",{rejectUnauthorized:false},res=>process.exit(res.statusCode===200?0:1)); r.on(\"error\",()=>process.exit(1)); r.setTimeout(4000,()=>{r.destroy();process.exit(1)});"' \
  --health-interval=30s \
  --health-timeout=5s \
  --health-start-period=15s \
  --health-retries=3 \
  -p "${POC_BIND_IP}:${POC_PORT}:8443/tcp" \
  -e "CERT_ALT_NAME=${CERT_ALT_NAME}" \
  "${IMAGE_NAME}"

echo "PoC disponible en https://${POC_BIND_IP}:${POC_PORT}"
