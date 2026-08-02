FROM rust:1.85-bookworm AS wasm-builder

RUN cargo install wasm-pack --version 0.13.1 --locked
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

ENV PORT=8443
EXPOSE 8443 9000
ENTRYPOINT ["/app/helpers/shell/start_stack.sh"]
