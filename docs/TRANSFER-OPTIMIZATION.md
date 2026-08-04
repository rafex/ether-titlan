# Tōna Transfer: optimización del canal óptico

## Objetivo

Reducir el tiempo real de transferencia sin sacrificar la legibilidad del QR en cámaras móviles. El cuello de botella no es Rust/WASM: es la combinación de capacidad del símbolo QR, refresco de pantalla, exposición/enfoque de cámara y tiempo de `jsQR`.

La cifra de 60 FPS es una cota del emisor, no una garantía de 60 paquetes decodificados por segundo. En un móvil real, el receptor suele validar muchos menos fotogramas.

## Cambios implementados

### 1. Transporte binario

La ruta activa dejó de usar Base64 para los paquetes de datos:

```text
binario → Deflate adaptativo → paquete binario → QR byte mode
```

La librería local `qrcode` recibe segmentos `{ data: Uint8Array, mode: "byte" }`, y `jsQR` entrega `binaryData`. Se conserva un QR textual pequeño para las solicitudes de control.

El paquete binario contiene:

- versión/magic `TN2`;
- tipo: cabecera, datos o reparación FEC;
- identificador FNV-1a de archivo + nombre para control de transferencia;
- digest SHA-256 de archivo + nombre para validar la reconstrucción;
- índice de datos o grupo FEC;
- payload comprimido y nombre original en la cabecera.

Esto elimina el 33% aproximado de expansión de Base64 y evita repetir prefijos de texto en cada paquete.

### 2. Compresión adaptativa

WASM calcula Deflate y compara su tamaño con el original:

- si Deflate reduce el archivo, transmite Deflate;
- si no reduce el archivo, transmite bytes originales;
- el codec elegido queda registrado en la cabecera.

Así JPEG, PNG, ZIP, MP4, PDF comprimidos y archivos cifrados no pagan el coste de una compresión inútil.

### 3. FEC XOR sistemático en TN2

Por defecto se genera una reparación XOR por cada 8 paquetes de datos. El receptor puede reconstruir automáticamente un paquete perdido de un grupo si recibió los otros siete y la reparación.

La FEC se puede desactivar desde el emisor para minimizar tráfico cuando el enlace es estable. La retransmisión selectiva sigue disponible mediante el QR `REQUEST|checksum|rangos`.

FEC no reemplaza Fountain Codes: es una protección sencilla y determinista. Si faltan dos paquetes del mismo grupo, el receptor los muestra como faltantes y solicita sus índices.

### 4. Estrategia Fountain/LT seleccionable

La aplicación también implementa `TNF`, una variante Fountain/LT de ventana acotada:

- cada ventana contiene hasta 32 símbolos fuente;
- los símbolos fuente se envían sistemáticamente para conservar una ruta rápida cuando el canal es bueno;
- los símbolos de reparación son ecuaciones XOR con máscaras pseudoaleatorias;
- el receptor realiza eliminación Gaussiana sobre cada ventana y recupera símbolos perdidos;
- el overhead de reparación se elige en el frontend: 10%, 20%, 30% o 40%;
- no necesita NACK ni que el emisor lea una cámara de retorno.

El decodificador conserva el mapa de símbolos resueltos, tolera orden arbitrario y descarta duplicados. La estrategia no pretende ser RaptorQ: RaptorQ requiere un codec estandarizado con sus propios parámetros, distribución de grados y formato de símbolos. `TNF` deja esa interfaz abierta sin mezclar sus paquetes con `TN2`.

La regla práctica es elegir `TN2` en una instalación controlada donde se pueda pedir retransmisión, y `TNF` cuando el receptor tenga movimiento o sólo exista un sentido óptico. Un overhead alto aumenta la tolerancia, pero también aumenta los QR totales y el tiempo de transmisión.

### 5. Repetición adaptativa

El emisor ofrece tres modos:

- **Automática:** 2 fotogramas para paquetes pequeños, 3 para payload grande, 4 durante recuperación.
- **Rápida:** 1 fotograma por paquete.
- **Confiable:** 4 fotogramas por paquete.

Cuando llega una solicitud de faltantes, el emisor cambia a modo de recuperación y sólo construye una cola con esos índices, sin repetir todos los datos.

### 6. Escaneo por frame real y ROI

El receptor usa `HTMLVideoElement.requestVideoFrameCallback()` cuando el navegador lo soporta; `requestAnimationFrame()` queda como fallback. Esto evita analizar varias veces el mismo frame de cámara.

Después de detectar un QR, se calcula un rectángulo alrededor de sus cuatro esquinas y los siguientes escaneos usan esa región de interés. Si se pierden diez detecciones consecutivas, el receptor vuelve a analizar todo el vídeo.

### 7. Parámetros de legibilidad

La densidad binaria se configura sin recompilar WASM:

- 400–1800 bytes por paquete (`600` por defecto);
- corrección QR `L` o `M`;
- FEC activada o desactivada;
- repetición automática, rápida o confiable.

Valores menores son preferibles cuando el celular está lejos, hay reflejos o el monitor tiene bajo brillo.

## Comparación de alternativas

### Índices + FEC + NACK óptico — opción actual

Ventajas:

- fácil de depurar y visualizar;
- muestra exactamente qué índices faltan;
- recupera una pérdida aislada sin esperar retransmisión;
- retransmite sólo rangos faltantes.

Costes:

- FEC añade aproximadamente 12.5% con grupos de ocho;
- NACK requiere que el emisor lea el QR del receptor;
- no recupera dos pérdidas dentro del mismo grupo.

### Fountain/LT y RaptorQ

La implementación actual genera combinaciones XOR de bloques dentro de ventanas y el receptor reconstruye el archivo cuando obtiene suficientes ecuaciones independientes. Es Fountain/LT simplificado, no RaptorQ completo.

Ventajas:

- no requiere canal de retorno;
- tolera pérdidas y movimiento de cámara;
- no necesita conocer todos los índices para avanzar.

Costes:

- más CPU y memoria en WASM;
- más complejidad matemática y de pruebas;
- requiere tráfico de reparación adicional;
- la visualización de “qué paquete falta” deja de ser tan directa.

Para archivos de 1.5 MiB, ambas rutas son válidas según el canal. Un siguiente adaptador RaptorQ puede reutilizar la interfaz de preparación, el formato de metadatos, el Worker y la telemetría sin cambiar la UX. SHA-256 detecta corrupción, pero no proporciona confidencialidad.

## Medición recomendada

Para comparar dispositivos registrar:

```text
bytes originales
bytes comprimidos
bytes por paquete
paquetes de datos
paquetes FEC
frames de vídeo
QR detectados
duplicados
paquetes recuperados por FEC
paquetes faltantes finales
tiempo total
```

El contador mostrado en ambos extremos mide tiempo transcurrido, no velocidad de red. El backend Python sólo sirve la aplicación y el endpoint de salud; el archivo se mantiene en WASM y cruza pantalla/cámara.

El emisor prepara los paquetes en un Web Worker. El receptor persiste cada paquete válido en IndexedDB para restaurar una sesión parcial después de una recarga; el control **Reiniciar recepción** limpia esa sesión.

## Pruebas

El protocolo WASM cubre:

- round trip binario con compresión adaptativa;
- datos antes de la cabecera;
- pérdida de un paquete recuperada mediante FEC;
- duplicados, orden arbitrario y mapa de faltantes;
- validación de checksum, SHA-256, tamaño y nombre;
- rechazo de payload manipulado.
- recuperación Fountain/LT de varias pérdidas dentro de una ventana.

Ejecutar:

```bash
cargo test --manifest-path frontend/wasm/Cargo.toml
cd frontend/wasm && wasm-pack build --target web --release --out-dir ../pkg
node --check frontend/main.js
CONTAINER_CONNECTION=bastion-tunnel make ci
```
