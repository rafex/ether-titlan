# QR Light Transfer

PoC de transferencia de archivos binarios de hasta **1.5 MiB** mediante códigos QR animados. El archivo se transporta ópticamente entre pantalla y cámara; no se envía por el backend.

## Separación frontend/backend

```text
frontend/                  Node.js + HTML/CSS/JavaScript vanilla
  server.mjs               HTTPS estático y proxy /api/*
  main.js                  Cámara, Canvas, QR y WASM
  wasm/                    Rust 2024 + wasm-bindgen
  vendor/                  qrcode y jsQR locales
backend/                   Python estándar
  server.py                API mínima /api/health y /api/info
helpers/
  shell/                   Acciones ejecutables del entorno
  mk/                      Fragmentos de construcción para Make
  just/                    Tareas operativas para Just
  python/                  Validaciones Python
```

El frontend Node.js termina TLS en el puerto `8443` y hace proxy de `/api/*` hacia el backend Python en `127.0.0.1:9000`. La API solo expone estado del servicio; el contenido del archivo permanece en WASM y en la cámara/pantalla.

## Responsabilidades de Make y Just

- `Makefile`: construcción y validación únicamente (`wasm`, `check`, `build`, `container`). Sus recetas viven en `helpers/mk/build.mk` y llaman scripts de `helpers/shell/`.
- `Justfile`: operación del entorno (`dev`, `start`, `stop`, `logs`, `clean`, `poc-up`, `poc-down`, `poc-logs`). Sus recetas viven en `helpers/just/tasks.just` y pueden llamar a Make cuando necesitan construir.
- Make nunca invoca Just.

Ejemplos:

```bash
# Construir WASM y validar frontend/backend
make build

# Arrancar frontend Node.js + backend Python + HTTPS local
just dev

# Operaciones disponibles
just start
just stop
just logs
just clean
just poc-up
just poc-down
just poc-logs

# Construir y ejecutar el CI local contenedorizado
CONTAINER_CONNECTION=podman-machine-default just ci

# Construir imagen Podman desde Just
just container
```

También se puede construir el WASM directamente:

```bash
cd frontend/wasm
wasm-pack build --target web --release --out-dir ../pkg
```

Requisitos locales: Rust 1.85+, `wasm-pack`, Node.js 20+, npm, Python 3, OpenSSL, Make y Just.

`make ci` construye un contenedor CI aislado, ejecuta `cargo fmt`, `cargo test`, `wasm-pack`, las validaciones Node/Python y deja los artefactos en `ci/artifacts/`. No requiere instalar Rust, Node o Python para ejecutar el CI en el host.

## Podman + HTTPS

El `Containerfile` compila Rust/WASM en una etapa y ejecuta Node.js + Python en la imagen final:

```bash
CONTAINER_CONNECTION=podman-machine-default make container

POC_BIND_IP=192.168.3.175 \
POC_PORT=30000 \
CONTAINER_CONNECTION=podman-machine-default just poc-up
```

La tarea publica `192.168.3.175:30000/tcp` hacia el HTTPS interno del contenedor (`8443/tcp`), dentro del rango permitido por UFW. Abre `https://192.168.3.175:30000` en el celular, acepta el certificado autofirmado y selecciona **Receptor**. En la computadora selecciona **Emisor**, carga el archivo e inicia la emisión. Para otra interfaz o puerto del rango `30000:30099`, sobrescribe `POC_BIND_IP`, `POC_PORT` y `CERT_ALT_NAME`.

El acceso inicial a la página usa la red local, pero el archivo no pasa por ella: la transferencia se realiza mediante luz, Canvas y cámara.

## Notas del protocolo

Rust divide el archivo en un paquete de cabecera y paquetes de datos indexados. El receptor tolera orden arbitrario, duplicados y pérdida de fotogramas. Cada paquete Base64 queda por debajo de 1,800 bytes y la cabecera conserva nombre, tamaño, cantidad de paquetes y checksum FNV-1a.

El QR selecciona automáticamente la versión necesaria con corrección `M`. Una carga de 1,800 caracteres no cabe físicamente en QR versión 5/6; forzar esas versiones reduciría demasiado la capacidad y haría inviable el límite de 1.5 MiB.
