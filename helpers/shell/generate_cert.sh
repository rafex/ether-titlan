#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CERT_DIR="${CERT_DIR:-${ROOT_DIR}/certs}"
CERT_FILE="${CERT_DIR}/server.crt"
KEY_FILE="${CERT_DIR}/server.key"
CERT_ALT_NAME="${CERT_ALT_NAME:-localhost}"

mkdir -p "${CERT_DIR}"
if [[ -s "${CERT_FILE}" && -s "${KEY_FILE}" ]]; then
  exit 0
fi

if [[ "${CERT_ALT_NAME}" =~ ^[0-9.]+$ ]]; then
  SAN="DNS:localhost,IP:127.0.0.1,IP:${CERT_ALT_NAME}"
else
  SAN="DNS:localhost,DNS:${CERT_ALT_NAME},IP:127.0.0.1"
fi

openssl req -x509 -nodes -newkey rsa:2048 -sha256 -days 365 \
  -keyout "${KEY_FILE}" -out "${CERT_FILE}" \
  -subj "/CN=${CERT_ALT_NAME}" \
  -addext "subjectAltName=${SAN}" \
  -addext "keyUsage=digitalSignature,keyEncipherment" \
  -addext "extendedKeyUsage=serverAuth" \
  >/dev/null 2>&1
chmod 600 "${KEY_FILE}"
echo "Generated self-signed certificate for ${SAN}." >&2
