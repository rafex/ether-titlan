FROM rust:1.91-bookworm AS wasm-builder

RUN cargo install wasm-pack --version 0.15.0 --locked
WORKDIR /build/frontend/wasm
COPY frontend/wasm/Cargo.toml ./
COPY frontend/wasm/Cargo.lock ./
COPY frontend/wasm/src ./src
RUN wasm-pack build --target web --out-dir ../pkg --release

FROM node:22-bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash openssl python3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY frontend ./frontend
COPY --from=wasm-builder /build/frontend/pkg ./frontend/pkg
COPY backend ./backend
COPY helpers ./helpers
RUN chmod +x ./helpers/shell/*.sh ./helpers/python/*.py

RUN groupadd --system --gid 10001 tona \
    && useradd --system --uid 10001 --gid 10001 --home-dir /app --shell /usr/sbin/nologin tona \
    && chown -R tona:tona /app

ENV PORT=8443
EXPOSE 8443 9000
USER tona
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD node -e "const https=require('https'); const r=https.get('https://127.0.0.1:8443/api/health',{rejectUnauthorized:false},res=>process.exit(res.statusCode===200?0:1)); r.on('error',()=>process.exit(1)); r.setTimeout(4000,()=>{r.destroy();process.exit(1)});"
ENTRYPOINT ["/app/helpers/shell/start_stack.sh"]
