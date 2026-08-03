# Tōna Transfer

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
CONTAINER_CONNECTION=bastion-tunnel just ci

# Construir imagen Podman desde Just
just container
```

También se puede construir el WASM directamente:

```bash
cd frontend/wasm
wasm-pack build --target web --release --out-dir ../pkg
```

Requisitos locales: Rust 1.91+, `wasm-pack 0.15.0`, Node.js 20+, npm, Python 3, OpenSSL, Make y Just.

`make ci` construye un contenedor CI aislado, ejecuta `cargo fmt`, `cargo test`, `wasm-pack`, las validaciones Node/Python y deja los artefactos en `ci/artifacts/`. No requiere instalar Rust, Node o Python para ejecutar el CI en el host.

## Podman + HTTPS

El `Containerfile` compila Rust/WASM en una etapa y ejecuta Node.js + Python en la imagen final:

```bash
CONTAINER_CONNECTION=bastion-tunnel make container

POC_BIND_IP=192.168.3.175 \
POC_PORT=30000 \
CONTAINER_CONNECTION=bastion-tunnel just poc-up
```

La tarea publica `192.168.3.175:30000/tcp` hacia el HTTPS interno del contenedor (`8443/tcp`), dentro del rango permitido por UFW. Abre `https://192.168.3.175:30000` en el celular, acepta el certificado autofirmado y selecciona **Receptor**. En la computadora selecciona **Emisor**, carga el archivo e inicia la emisión. Para otra interfaz o puerto del rango `30000:30099`, sobrescribe `POC_BIND_IP`, `POC_PORT` y `CERT_ALT_NAME`.

El acceso inicial a la página usa la red local, pero el archivo no pasa por ella: la transferencia se realiza mediante luz, Canvas y cámara.

## Notas del protocolo

La ruta activa usa el protocolo binario `TN2`: Deflate adaptativo (o bytes originales si comprimir no reduce), paquetes QR en modo byte, cabecera compacta, índices y reparaciones XOR FEC. El receptor tolera orden arbitrario, duplicados, paquetes antes de la cabecera y una pérdida por grupo FEC; concatena, descomprime cuando corresponde y valida checksum/tamaño antes de entregar el archivo.

El QR selecciona automáticamente la versión necesaria con corrección `L` o `M`, usa un margen de cuatro módulos y repite cada paquete según el modo seleccionado. La densidad binaria se puede ajustar entre `400–1800` bytes (`600` por defecto) desde la pestaña **Emisor** sin recompilar. La cabecera se emite al inicio, mitad y final de cada ciclo para que el receptor pueda incorporarse tarde o recuperar metadatos.

El receptor expone el nombre y extensión, tamaño, checksum, contador transcurrido y un mapa visual de paquetes: verde significa recibido y rojo faltante. También genera una solicitud óptica `REQUEST|checksum|rangos`; el emisor puede leerla con **Leer solicitud del receptor** y reconstruir su cola para retransmitir solamente esos índices. Si la solicitud no cabe en un QR, se puede escribir el mismo rango manualmente, por ejemplo `0-3,8,10-12`.

### Compresión y retransmisión

No se comprime Base64 por paquete. Base64 añade aproximadamente 33% de tamaño y al convertir primero el binario a texto se pierde parte de la redundancia que Deflate puede aprovechar. La ruta activa envía directamente bytes QR; Deflate se aplica al archivo completo sólo cuando reduce su tamaño y el codec se registra en la cabecera.

La propuesta de comprimir cada trozo con gzip/LZMA/XZ tiene estas desventajas para esta PoC:

- Cada trozo independiente repite cabeceras y suele comprimir peor.
- Gzip/Deflate por trozo aumenta el número de estados y paquetes de control.
- LZMA/XZ puede mejorar la ratio en texto repetitivo, pero eleva el tamaño del WASM, CPU, memoria y latencia móvil.
- Imágenes JPEG, ZIP, PDF comprimidos o datos cifrados pueden no reducirse más.

Para esta arquitectura hay dos caminos sólidos:

1. **Índices + bitmap + FEC + NACK óptico (implementado):** bytes QR, compresión adaptativa, reparaciones XOR, mapa de faltantes y un QR de solicitud que viaja de vuelta desde el receptor al emisor. Es sencillo de inspeccionar, permite reanudar y retransmite sólo lo perdido, pero requiere que el emisor tenga cámara y que ambos dispositivos puedan apuntarse alternativamente.
2. **Fountain/Raptor simplificado (siguiente evolución):** el emisor transmite combinaciones XOR de bloques con una semilla; el receptor reconstruye cuando obtiene suficientes combinaciones independientes, sin canal de retorno. Tolera mejor pérdidas y movimiento, pero requiere más protocolo, memoria y pruebas matemáticas; ya no se puede mostrar un índice recibido de forma tan directa.

La solución actual conserva la primera alternativa porque es verificable para archivos de hasta 1.5 MiB y permite mostrar exactamente qué paquetes faltan. El contador de tiempo es informativo y se inicia al comenzar la emisión o al activar la cámara.

Las optimizaciones de transporte binario, FEC, compresión adaptativa, repetición adaptativa, ROI y `requestVideoFrameCallback` están descritas en [docs/TRANSFER-OPTIMIZATION.md](docs/TRANSFER-OPTIMIZATION.md).

### Webcam en computadora

Para usar una webcam de escritorio, abre la dirección HTTPS completa, no una URL HTTP:

```text
https://192.168.3.175:30000
```

Acepta el certificado autofirmado y después pulsa **Activar cámara**. El receptor muestra las webcams disponibles después de conceder el permiso y permite seleccionar una explícitamente. Si la configuración orientada a móvil no es compatible con el dispositivo, el frontend reintenta con `video: true`.

Si el navegador indica que `getUserMedia` no está disponible, revisa el panel **Diagnóstico del receptor**: una URL `http://192.168.3.175:30000` no es un contexto seguro y el navegador bloqueará la webcam. También verifica que otra aplicación no esté usando exclusivamente la cámara.
