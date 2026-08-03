import init, {
  binary_receiver_control_packet,
  binary_receiver_filename,
  binary_receiver_info,
  binary_receiver_missing_ranges,
  binary_receiver_packet_map,
  binary_receiver_progress,
  prepare_binary_packets,
  process_binary_packet,
  reset_binary_receiver,
} from "./pkg/qr_file_transfer.js";

const MAX_FILE_BYTES = 1.5 * 1024 * 1024;
const QR_SIZE = 640;
const DEFAULT_CHUNK_BYTES = 800;
const MAX_SCAN_WIDTH = 1280;

const $ = (id) => document.getElementById(id);
const sender = {
  packets: [],
  metadata: null,
  queue: [],
  selectedDataIndexes: null,
  file: null,
  frame: null,
  index: 0,
  rendering: false,
  running: false,
  packetRendered: false,
  packetFrames: 0,
  repeatMode: "auto",
  recoveryMode: false,
  clock: { startedAt: null, elapsedMs: 0, timer: null },
};
const receiver = {
  stream: null,
  frame: null,
  running: false,
  lastPacket: "",
  logLines: [],
  stats: { frames: 0, qrDetections: 0, duplicates: 0, packets: 0 },
  controlRendering: false,
  controlPayload: "",
  roi: null,
  roiMisses: 0,
  scanWithVideoFrameCallback: false,
  clock: { startedAt: null, elapsedMs: 0, timer: null },
};
const senderControl = {
  stream: null,
  frame: null,
  running: false,
  lastRequest: "",
};

const senderCanvas = $("sender-canvas");
const scanCanvas = $("scan-canvas");
const scanContext = scanCanvas.getContext("2d", { willReadFrequently: true });
const senderControlScanCanvas = $("sender-control-scan-canvas");
const senderControlScanContext = senderControlScanCanvas.getContext("2d", { willReadFrequently: true });
const receiverPacketMapCanvas = $("receiver-packet-map");
const receiverPacketMapContext = receiverPacketMapCanvas.getContext("2d");
const receiverControlCanvas = $("receiver-control-canvas");

function setStatus(element, message, isError = false) {
  element.textContent = message;
  element.classList.toggle("error", isError);
}

function formatElapsed(milliseconds) {
  const totalSeconds = Math.floor(milliseconds / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  return hours > 0
    ? `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`
    : `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

function clockValue(clock) {
  return clock.startedAt === null ? clock.elapsedMs : clock.elapsedMs + performance.now() - clock.startedAt;
}

function renderClock(clock, elementId) {
  $(elementId).textContent = formatElapsed(clockValue(clock));
}

function resetClock(clock, elementId) {
  if (clock.timer) clearInterval(clock.timer);
  clock.timer = null;
  clock.startedAt = null;
  clock.elapsedMs = 0;
  renderClock(clock, elementId);
}

function startClock(clock, elementId) {
  if (clock.startedAt !== null) return;
  clock.startedAt = performance.now();
  renderClock(clock, elementId);
  clock.timer = setInterval(() => renderClock(clock, elementId), 250);
}

function pauseClock(clock, elementId) {
  if (clock.startedAt !== null) {
    clock.elapsedMs += performance.now() - clock.startedAt;
    clock.startedAt = null;
  }
  if (clock.timer) clearInterval(clock.timer);
  clock.timer = null;
  renderClock(clock, elementId);
}

function receiverLog(message, details = undefined) {
  const suffix = details ? ` ${JSON.stringify(details)}` : "";
  const line = `${new Date().toISOString()} ${message}${suffix}`;
  receiver.logLines.push(line);
  receiver.logLines = receiver.logLines.slice(-12);
  const debug = $("receiver-debug");
  if (debug) debug.textContent = receiver.logLines.join("\n");
  if (details) console.info(`[Tōna receiver] ${message}`, details);
  else console.info(`[Tōna receiver] ${message}`);
}

function qrSettings() {
  return {
    chunkBytes: Number($("qr-chunk-size").value),
    fecGroupSize: Number($("qr-fec-group").value),
    repeatMode: $("qr-repeat-mode").value,
    errorCorrectionLevel: $("qr-error-correction").value,
  };
}

function qrOptions() {
  return {
    errorCorrectionLevel: qrSettings().errorCorrectionLevel,
    margin: 4,
    width: QR_SIZE,
    color: { dark: "#000000", light: "#ffffff" },
  };
}

function updateQrSettingsLabel() {
  const settings = qrSettings();
  const readability = settings.chunkBytes <= 800 ? "alta" : settings.chunkBytes <= 1200 ? "media" : "baja";
  const fec = settings.fecGroupSize ? `FEC 1/${settings.fecGroupSize}` : "sin FEC";
  $("qr-settings-label").textContent = `${settings.chunkBytes} bytes por paquete · legibilidad ${readability} · ${fec} · corrección ${settings.errorCorrectionLevel}`;
}

function qrBinaryPayload(packet) {
  return [{ data: packet, mode: "byte" }];
}

function packetChecksum(packet) {
  if (!packet || packet.length < 15) return "";
  return Array.from(packet.slice(7, 15), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function packetType(packet) {
  return packet?.[3];
}

function senderDataPacketCount() {
  return sender.metadata?.dataPackets || 0;
}

function senderParityPacketCount() {
  return sender.metadata?.parityPackets || 0;
}

function holdFramesFor(entry) {
  const mode = sender.repeatMode;
  if (mode === "fast") return 1;
  if (mode === "reliable") return 4;
  if (sender.recoveryMode) return 4;
  if (entry?.kind === "header" || entry?.kind === "parity") return 2;
  return entry?.payload?.length > 1050 ? 3 : 2;
}

async function checkBackend() {
  try {
    const response = await fetch("/api/health");
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const health = await response.json();
    $("backend-status").textContent = `Backend: ${health.service} — transporte ${health.transport}.`;
  } catch (error) {
    $("backend-status").textContent = `Backend Python no disponible: ${error.message}`;
    $("backend-status").classList.add("error");
  }
}

function updateSenderProgress() {
  const totalData = senderDataPacketCount();
  const queueTotal = sender.queue.length;
  const current = queueTotal ? sender.index + 1 : 0;
  const entry = sender.queue[sender.index];
  $("sender-progress").max = Math.max(queueTotal, 1);
  $("sender-progress").value = current;
  $("sender-progress-label").textContent = entry?.kind === "header"
    ? `Cabecera · envío ${current} de ${queueTotal}`
    : entry?.kind === "parity"
      ? `FEC reparación · envío ${current} de ${queueTotal}`
    : `Paquete ${entry ? entry.dataIndex + 1 : 0} de ${totalData} · envío ${current} de ${queueTotal}`;
  const selected = sender.selectedDataIndexes ? sender.selectedDataIndexes.length : totalData;
  const parity = sender.selectedDataIndexes ? 0 : senderParityPacketCount();
  $("sender-packet-meta").innerHTML = `Datos: ${selected} de ${totalData} · reparación FEC: ${parity} · tiempo: <time id="sender-elapsed">${formatElapsed(clockValue(sender.clock))}</time>`;
}

function allSenderDataIndexes() {
  return Array.from({ length: senderDataPacketCount() }, (_, index) => index);
}

function buildSenderQueue() {
  const indexes = sender.selectedDataIndexes || allSenderDataIndexes();
  const header = { kind: "header", payload: sender.packets[0], dataIndex: -1 };
  if (!sender.packets.length) {
    sender.queue = [];
    return;
  }

  const body = indexes.map((dataIndex) => ({ kind: "data", payload: sender.packets[dataIndex + 1], dataIndex }));
  if (!sender.selectedDataIndexes && senderParityPacketCount()) {
    const parityStart = senderDataPacketCount() + 1;
    for (let index = 0; index < senderParityPacketCount(); index += 1) {
      body.push({ kind: "parity", payload: sender.packets[parityStart + index], dataIndex: -1 });
    }
  }
  sender.queue = [header];
  body.forEach((entry, position) => {
    if (body.length > 1 && position === Math.ceil(body.length / 2)) sender.queue.push(header);
    sender.queue.push(entry);
  });
  sender.queue.push(header);
  sender.index = 0;
  sender.packetRendered = false;
  sender.packetFrames = 0;
  updateSenderProgress();
}

function parseMissingRanges(value, total) {
  const normalized = value.trim();
  if (!normalized) return [];
  const result = new Set();
  for (const token of normalized.split(",").map((part) => part.trim()).filter(Boolean)) {
    const match = /^(\d+)(?:-(\d+))?$/.exec(token);
    if (!match) throw new Error(`Rango inválido: ${token}`);
    const start = Number(match[1]);
    const end = match[2] === undefined ? start : Number(match[2]);
    if (start > end || end >= total) throw new Error(`Rango fuera de límites: ${token}`);
    for (let index = start; index <= end; index += 1) result.add(index);
  }
  return [...result].sort((a, b) => a - b);
}

function applyMissingSelection(value, source = "manual") {
  const total = senderDataPacketCount();
  if (!sender.packets.length) throw new Error("Selecciona un archivo antes de aplicar faltantes.");
  const indexes = parseMissingRanges(value, total);
  if (!indexes.length) {
    sender.selectedDataIndexes = null;
    sender.recoveryMode = false;
    $("sender-missing-ranges").value = "";
    setStatus($("sender-file"), `${sender.file.name} — emisión configurada para todos los paquetes.`);
  } else {
    sender.selectedDataIndexes = indexes;
    $("sender-missing-ranges").value = value.trim();
    setStatus($("sender-file"), `${sender.file.name} — retransmisión selectiva de ${indexes.length} paquetes (${source}).`);
  }
  buildSenderQueue();
  updateSenderProgress();
}

function updateReceiverProgress() {
  const [received, total] = binary_receiver_progress();
  $("receiver-progress").max = Math.max(total, 1);
  $("receiver-progress").value = received;
  $("receiver-progress-label").textContent = `Paquetes recibidos: ${received} de ${total}`;
  const info = JSON.parse(binary_receiver_info() || "{}");
  if (info.filename) {
    const size = info.file_size ? `${(info.file_size / 1024).toFixed(1)} KiB` : "tamaño desconocido";
    const compressed = info.compressed_size ? ` · comprimido ${(info.compressed_size / 1024).toFixed(1)} KiB` : "";
    const fec = info.fec_group_size ? ` · FEC recuperados ${info.recovered_packets || 0}` : "";
    $("receiver-file-meta").textContent = `Archivo: ${info.filename} · ${size}${compressed} · ${info.codec || "—"}${fec} · checksum ${info.checksum || "—"}`;
  } else {
    $("receiver-file-meta").textContent = "Archivo: pendiente de cabecera.";
  }
  const missing = binary_receiver_missing_ranges();
  const visibleMissing = missing.length > 600 ? `${missing.slice(0, 600)}…` : missing;
  $("receiver-missing-label").textContent = visibleMissing ? `Faltantes: ${visibleMissing}` : (total ? "Faltantes: ninguno" : "Faltantes: —");
  drawReceiverPacketMap(binary_receiver_packet_map());
  updateReceiverControlQr();
}

function drawReceiverPacketMap(packetMap) {
  const width = receiverPacketMapCanvas.width;
  const height = receiverPacketMapCanvas.height;
  receiverPacketMapContext.clearRect(0, 0, width, height);
  receiverPacketMapContext.fillStyle = "#080d18";
  receiverPacketMapContext.fillRect(0, 0, width, height);
  if (!packetMap.length) return;
  const columns = Math.min(width, packetMap.length);
  const slotWidth = width / columns;
  for (let column = 0; column < columns; column += 1) {
    const start = Math.floor(column * packetMap.length / columns);
    const end = Math.max(start + 1, Math.ceil((column + 1) * packetMap.length / columns));
    const receivedCount = packetMap.slice(start, end).reduce((sum, received) => sum + received, 0);
    receiverPacketMapContext.fillStyle = receivedCount === 0
      ? "#d95f59"
      : receivedCount === end - start ? "#36b37e" : "#e0a458";
    const x = Math.floor(column * slotWidth);
    const nextX = Math.max(x + 1, Math.ceil((column + 1) * slotWidth));
    receiverPacketMapContext.fillRect(x, 0, nextX - x, height);
  }
}

function updateReceiverControlQr() {
  if (!window.QRCode || receiver.controlRendering) return;
  const payload = binary_receiver_control_packet();
  receiver.controlPayload = payload;
  if (!payload) {
    receiverControlCanvas.getContext("2d").clearRect(0, 0, receiverControlCanvas.width, receiverControlCanvas.height);
    $("receiver-control-status").textContent = "Aún no hay una cabecera recibida.";
    return;
  }
  receiver.controlRendering = true;
  try {
    window.QRCode.toCanvas(receiverControlCanvas, payload, {
      errorCorrectionLevel: "L",
      margin: 4,
      width: 320,
      color: { dark: "#000000", light: "#ffffff" },
    }, (error) => {
      receiver.controlRendering = false;
      if (error) {
        $("receiver-control-status").textContent = `La solicitud es demasiado grande para un QR: ${error.message}`;
        return;
      }
      const missing = binary_receiver_missing_ranges();
      const visible = missing.length > 600 ? `${missing.slice(0, 600)}…` : missing;
      $("receiver-control-status").textContent = visible
        ? `Solicitud lista: ${visible}`
        : "No hay paquetes faltantes.";
    });
  } catch (error) {
    receiver.controlRendering = false;
    $("receiver-control-status").textContent = `La solicitud es demasiado grande para un QR: ${error.message}`;
  }
}

function stopSender() {
  sender.running = false;
  sender.frame = null;
  sender.rendering = false;
  sender.packetRendered = false;
  sender.packetFrames = 0;
  pauseClock(sender.clock, "sender-elapsed");
  $("start-sender").disabled = !sender.packets.length;
  $("stop-sender").disabled = true;
}

function renderSenderFrame() {
  if (!sender.running) return;
  sender.frame = requestAnimationFrame(renderSenderFrame);
  if (sender.rendering || !sender.queue.length || !window.QRCode) return;

  if (sender.packetRendered) {
    sender.packetFrames += 1;
    if (sender.packetFrames < holdFramesFor(sender.queue[sender.index])) return;
    sender.packetRendered = false;
    sender.packetFrames = 0;
    sender.index = (sender.index + 1) % sender.queue.length;
    updateSenderProgress();
  }

  const payload = sender.queue[sender.index].payload;
  sender.rendering = true;
  window.QRCode.toCanvas(senderCanvas, qrBinaryPayload(payload), qrOptions(), (error) => {
    sender.rendering = false;
    if (error) {
      stopSender();
      setStatus($("sender-file"), `No se pudo generar el QR: ${error.message}`, true);
      return;
    }
    sender.packetRendered = true;
    sender.packetFrames = 0;
    updateSenderProgress();
  });
}

function startSender() {
  if (!sender.packets.length) return;
  buildSenderQueue();
  sender.running = true;
  sender.index = 0;
  sender.packetRendered = false;
  sender.packetFrames = 0;
  $("start-sender").disabled = true;
  $("stop-sender").disabled = false;
  startClock(sender.clock, "sender-elapsed");
  setStatus($("sender-file"), `${sender.file.name} — emisión activa, QR repetido para captura móvil.`);
  renderSenderFrame();
}

async function loadFile(file) {
  stopSender();
  sender.packets = [];
  sender.metadata = null;
  sender.queue = [];
  sender.selectedDataIndexes = null;
  sender.recoveryMode = false;
  sender.file = null;
  resetClock(sender.clock, "sender-elapsed");
  $("sender-missing-ranges").value = "";
  updateSenderProgress();

  if (!file) return;
  if (file.size > MAX_FILE_BYTES) {
    setStatus($("sender-file"), "El archivo supera el límite de 1.5 MiB.", true);
    return;
  }

  try {
    const buffer = await file.arrayBuffer();
    const settings = qrSettings();
    sender.packets = Array.from(prepare_binary_packets(
      new Uint8Array(buffer),
      file.name,
      settings.chunkBytes,
      settings.fecGroupSize,
    ), (packet) => new Uint8Array(packet));
    if (!sender.packets.length) throw new Error("WASM rechazó el archivo o el nombre es demasiado largo.");
    sender.file = file;
    const header = sender.packets[0];
    sender.metadata = {
      checksum: packetChecksum(header),
      dataPackets: new DataView(header.buffer, header.byteOffset, header.byteLength).getUint32(25),
      chunkBytes: new DataView(header.buffer, header.byteOffset, header.byteLength).getUint16(23),
      fecGroupSize: header[5],
      parityPackets: sender.packets.filter((packet) => packetType(packet) === 2).length,
      codec: header[4] === 1 ? "deflate" : "raw",
    };
    buildSenderQueue();
    $("start-sender").disabled = false;
    setStatus($("sender-file"), `${file.name} — ${(file.size / 1024).toFixed(1)} KiB listo con ${sender.metadata.dataPackets} paquetes binarios de ${settings.chunkBytes} bytes (${sender.metadata.codec}).`);
    updateSenderProgress();
  } catch (error) {
    setStatus($("sender-file"), `No se pudo preparar el archivo: ${error.message}`, true);
  }
}

function handleReceiverRequest(payload) {
  const fields = payload.split("|");
  if (fields.length !== 3 || fields[0] !== "REQUEST") return false;
  if (!sender.packets.length) {
    setStatus($("sender-file"), "Solicitud recibida; selecciona primero el archivo original.", true);
    return true;
  }
  if (fields[1] !== sender.metadata?.checksum) {
    setStatus($("sender-file"), "La solicitud pertenece a otro archivo.", true);
    return true;
  }
  try {
    sender.recoveryMode = Boolean(fields[2]);
    applyMissingSelection(fields[2], "QR del receptor");
    setStatus($("sender-file"), `${sender.file.name} — solicitud de faltantes aplicada; inicia o continúa la emisión.`);
    return true;
  } catch (error) {
    setStatus($("sender-file"), `Solicitud de faltantes inválida: ${error.message}`, true);
    return true;
  }
}

function stopSenderControl() {
  senderControl.running = false;
  if (senderControl.frame) cancelAnimationFrame(senderControl.frame);
  senderControl.frame = null;
  if (senderControl.stream) senderControl.stream.getTracks().forEach((track) => track.stop());
  senderControl.stream = null;
  $("sender-control-video").srcObject = null;
  $("sender-control-video").hidden = true;
  $("start-sender-control").disabled = false;
  $("stop-sender-control").disabled = true;
}

function scanSenderControlFrame() {
  if (!senderControl.running) return;
  senderControl.frame = requestAnimationFrame(scanSenderControlFrame);
  const video = $("sender-control-video");
  if (video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA || !video.videoWidth) return;
  const width = Math.min(800, video.videoWidth);
  const height = Math.round(video.videoHeight * (width / video.videoWidth));
  if (senderControlScanCanvas.width !== width || senderControlScanCanvas.height !== height) {
    senderControlScanCanvas.width = width;
    senderControlScanCanvas.height = height;
  }
  senderControlScanContext.imageSmoothingEnabled = false;
  senderControlScanContext.drawImage(video, 0, 0, width, height);
  const image = senderControlScanContext.getImageData(0, 0, width, height);
  let code;
  try {
    code = window.jsQR(image.data, image.width, image.height, { inversionAttempts: "attemptBoth" });
  } catch (error) {
    receiverLog("jsQR lanzó una excepción en el lector de solicitudes", { message: error.message });
    return;
  }
  if (!code?.data || code.data === senderControl.lastRequest) return;
  if (code.data.startsWith("REQUEST|")) {
    senderControl.lastRequest = code.data;
    receiverLog("Solicitud óptica recibida por el emisor", { length: code.data.length });
    handleReceiverRequest(code.data);
  }
}

async function startSenderControl() {
  if (!navigator.mediaDevices?.getUserMedia) {
    setStatus($("sender-file"), "Este navegador no expone getUserMedia para leer faltantes.", true);
    return;
  }
  try {
    senderControl.lastRequest = "";
    senderControl.stream = await navigator.mediaDevices.getUserMedia({
      audio: false,
      video: { facingMode: { ideal: "environment" }, width: { ideal: 800 }, height: { ideal: 600 } },
    });
    const video = $("sender-control-video");
    video.hidden = false;
    video.srcObject = senderControl.stream;
    await video.play();
    senderControl.running = true;
    $("start-sender-control").disabled = true;
    $("stop-sender-control").disabled = false;
    setStatus($("sender-file"), "Lector activo: apunta la cámara del emisor al QR de solicitud del receptor.");
    scanSenderControlFrame();
  } catch (error) {
    stopSenderControl();
    setStatus($("sender-file"), `No se pudo activar el lector de solicitudes: ${error.message}`, true);
  }
}

function stopReceiver() {
  receiver.running = false;
  const video = $("receiver-video");
  if (receiver.frame && receiver.frameKind === "video" && video.cancelVideoFrameCallback) {
    video.cancelVideoFrameCallback(receiver.frame);
  } else if (receiver.frame) {
    cancelAnimationFrame(receiver.frame);
  }
  receiver.frame = null;
  receiver.frameKind = null;
  if (receiver.stream) receiver.stream.getTracks().forEach((track) => track.stop());
  receiver.stream = null;
  $("receiver-video").srcObject = null;
  pauseClock(receiver.clock, "receiver-elapsed");
  $("start-receiver").disabled = false;
  $("stop-receiver").disabled = true;
  receiverLog("Cámara detenida");
}

function finishDownload(bytes) {
  const filename = binary_receiver_filename() || "archivo-recibido.bin";
  pauseClock(receiver.clock, "receiver-elapsed");
  const blob = new Blob([bytes], { type: "application/octet-stream" });
  const url = URL.createObjectURL(blob);
  const link = $("download-link");
  link.href = url;
  link.download = filename;
  link.hidden = false;
  link.textContent = `Descargar ${filename}`;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
  setStatus($("receiver-status"), `Transferencia completa: ${filename}.`);
  updateReceiverProgress();
}

function scheduleReceiverFrame() {
  if (!receiver.running) return;
  const video = $("receiver-video");
  if (video.requestVideoFrameCallback) {
    receiver.frameKind = "video";
    receiver.frame = video.requestVideoFrameCallback(() => scanReceiverFrame());
  } else {
    receiver.frameKind = "raf";
    receiver.frame = requestAnimationFrame(scanReceiverFrame);
  }
}

function updateReceiverRoi(code, crop) {
  const points = [
    code.location?.topLeftCorner,
    code.location?.topRightCorner,
    code.location?.bottomRightCorner,
    code.location?.bottomLeftCorner,
  ].filter(Boolean);
  if (points.length !== 4) return;
  const minX = Math.min(...points.map((point) => point.x));
  const maxX = Math.max(...points.map((point) => point.x));
  const minY = Math.min(...points.map((point) => point.y));
  const maxY = Math.max(...points.map((point) => point.y));
  const marginX = Math.max(18, (maxX - minX) * 0.12);
  const marginY = Math.max(18, (maxY - minY) * 0.12);
  const x = Math.max(0, minX - marginX);
  const y = Math.max(0, minY - marginY);
  const right = Math.min(crop.scanWidth, maxX + marginX);
  const bottom = Math.min(crop.scanHeight, maxY + marginY);
  receiver.roi = {
    x: crop.x + x / crop.scanWidth * crop.width,
    y: crop.y + y / crop.scanHeight * crop.height,
    width: (right - x) / crop.scanWidth * crop.width,
    height: (bottom - y) / crop.scanHeight * crop.height,
  };
  receiver.roiMisses = 0;
}

function qrFingerprint(code) {
  if (code.data?.startsWith("REQUEST|")) return code.data;
  const bytes = code.binaryData || [];
  return `binary:${bytes.length}:${Array.from(bytes.slice(0, 18)).join(",")}`;
}

function scanReceiverFrame() {
  if (!receiver.running) return;
  scheduleReceiverFrame();
  receiver.stats.frames += 1;
  const video = $("receiver-video");
  if (video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA || !video.videoWidth) {
    if (receiver.stats.frames % 60 === 0) receiverLog("Esperando frame de vídeo", {
      readyState: video.readyState,
      videoWidth: video.videoWidth,
      videoHeight: video.videoHeight,
    });
    return;
  }

  const activeRoi = receiver.roi || { x: 0, y: 0, width: 1, height: 1 };
  const crop = {
    x: Math.max(0, Math.min(1, activeRoi.x)),
    y: Math.max(0, Math.min(1, activeRoi.y)),
    width: Math.max(0.1, Math.min(1, activeRoi.width)),
    height: Math.max(0.1, Math.min(1, activeRoi.height)),
  };
  const cropWidth = Math.round(video.videoWidth * crop.width);
  const cropHeight = Math.round(video.videoHeight * crop.height);
  const scale = Math.min(1, MAX_SCAN_WIDTH / cropWidth);
  const width = Math.max(640, Math.round(cropWidth * scale));
  const height = Math.max(480, Math.round(cropHeight * (width / cropWidth)));
  if (scanCanvas.width !== width || scanCanvas.height !== height) {
    scanCanvas.width = width;
    scanCanvas.height = height;
  }
  scanContext.imageSmoothingEnabled = false;
  scanContext.drawImage(
    video,
    Math.round(video.videoWidth * crop.x),
    Math.round(video.videoHeight * crop.y),
    cropWidth,
    cropHeight,
    0,
    0,
    width,
    height,
  );
  const image = scanContext.getImageData(0, 0, width, height);
  let code;
  try {
    code = window.jsQR(image.data, image.width, image.height, { inversionAttempts: "attemptBoth" });
  } catch (error) {
    receiverLog("jsQR lanzó una excepción", { message: error.message });
    return;
  }
  if (!code?.binaryData?.length) {
    receiver.roiMisses += 1;
    if (receiver.roiMisses >= 10) receiver.roi = null;
    if (receiver.stats.frames % 60 === 0) receiverLog("Sin QR detectado", {
      frames: receiver.stats.frames,
      video: `${video.videoWidth}x${video.videoHeight}`,
      scan: `${width}x${height}`,
      roi: receiver.roi ? "activa" : "completa",
    });
    return;
  }

  receiver.stats.qrDetections += 1;
  updateReceiverRoi(code, {
    ...crop,
    scanWidth: width,
    scanHeight: height,
  });
  const fingerprint = qrFingerprint(code);
  if (fingerprint === receiver.lastPacket) {
    receiver.stats.duplicates += 1;
    return;
  }

  receiver.lastPacket = fingerprint;
  receiverLog("QR detectado", {
    length: code.binaryData.length,
    type: code.data?.startsWith("REQUEST|") ? "control" : "binario",
    detections: receiver.stats.qrDetections,
  });
  try {
    const assembled = process_binary_packet(new Uint8Array(code.binaryData));
    receiver.stats.packets += 1;
    updateReceiverProgress();
    receiverLog("Paquete enviado a WASM", {
      packets: receiver.stats.packets,
      progress: $("receiver-progress-label").textContent,
    });
    if (assembled !== null && assembled !== undefined) finishDownload(assembled);
  } catch (error) {
    receiverLog("WASM rechazó el paquete", { message: error.message });
    setStatus($("receiver-status"), `Paquete inválido: ${error.message}`, true);
  }
}

async function startReceiver() {
  if (!navigator.mediaDevices?.getUserMedia) {
    setStatus($("receiver-status"), "Este navegador no expone getUserMedia.", true);
    return;
  }
  if (!window.jsQR) {
    setStatus($("receiver-status"), "No se pudo cargar el decodificador jsQR.", true);
    return;
  }
  try {
    receiver.logLines = [];
    receiver.stats = { frames: 0, qrDetections: 0, duplicates: 0, packets: 0 };
    receiver.roi = null;
    receiver.roiMisses = 0;
    receiver.frameKind = null;
    resetClock(receiver.clock, "receiver-elapsed");
    receiverLog("Solicitando cámara");
    reset_binary_receiver();
    receiver.lastPacket = "";
    updateReceiverProgress();
    receiver.stream = await navigator.mediaDevices.getUserMedia({
      audio: false,
      video: {
        facingMode: { ideal: "environment" },
        width: { ideal: 1280 },
        height: { ideal: 720 },
        frameRate: { ideal: 30, max: 60 },
      },
    });
    const video = $("receiver-video");
    video.srcObject = receiver.stream;
    await video.play();
    const track = receiver.stream.getVideoTracks()[0];
    const settings = track?.getSettings?.() || {};
    const capabilities = track?.getCapabilities?.() || {};
    receiverLog("Cámara activa", {
      label: track?.label || "desconocida",
      settings,
      focusModes: capabilities.focusMode || [],
    });
    if (capabilities.focusMode?.includes("continuous")) {
      try {
        await track.applyConstraints({ advanced: [{ focusMode: "continuous" }] });
        receiverLog("Enfoque continuo solicitado");
      } catch (error) {
        receiverLog("No se pudo aplicar enfoque continuo", { message: error.message });
      }
    }
    receiver.running = true;
    startClock(receiver.clock, "receiver-elapsed");
    $("start-receiver").disabled = true;
    $("stop-receiver").disabled = false;
    setStatus($("receiver-status"), "Cámara activa. Buscando paquetes…");
    receiver.scanWithVideoFrameCallback = Boolean(video.requestVideoFrameCallback);
    receiverLog("Escaneo sincronizado con frames de vídeo", { requestVideoFrameCallback: receiver.scanWithVideoFrameCallback });
    scheduleReceiverFrame();
  } catch (error) {
    stopReceiver();
    setStatus($("receiver-status"), `No se pudo activar la cámara: ${error.message}`, true);
  }
}

function switchTab(tab) {
  document.querySelectorAll(".tab").forEach((button) => {
    button.setAttribute("aria-selected", String(button.dataset.tab === tab));
  });
  document.querySelectorAll(".panel").forEach((panel) => panel.classList.toggle("active", panel.id === `${tab}-panel`));
}

$("file-input").addEventListener("change", (event) => loadFile(event.target.files[0]));
$("drop-zone").addEventListener("dragover", (event) => { event.preventDefault(); $("drop-zone").classList.add("dragover"); });
$("drop-zone").addEventListener("dragleave", () => $("drop-zone").classList.remove("dragover"));
$("drop-zone").addEventListener("drop", (event) => {
  event.preventDefault();
  $("drop-zone").classList.remove("dragover");
  loadFile(event.dataTransfer.files[0]);
});
$("start-sender").addEventListener("click", startSender);
$("stop-sender").addEventListener("click", stopSender);
$("apply-missing").addEventListener("click", () => {
  try {
    applyMissingSelection($("sender-missing-ranges").value);
  } catch (error) {
    setStatus($("sender-file"), error.message, true);
  }
});
$("clear-missing").addEventListener("click", () => {
  try {
    applyMissingSelection("");
  } catch (error) {
    setStatus($("sender-file"), error.message, true);
  }
});
$("start-sender-control").addEventListener("click", startSenderControl);
$("stop-sender-control").addEventListener("click", stopSenderControl);
$("start-receiver").addEventListener("click", startReceiver);
$("stop-receiver").addEventListener("click", stopReceiver);
document.querySelectorAll(".tab").forEach((button) => button.addEventListener("click", () => switchTab(button.dataset.tab)));
$("qr-chunk-size").addEventListener("input", updateQrSettingsLabel);
$("qr-chunk-size").addEventListener("change", () => {
  if (sender.file) loadFile(sender.file);
});
$("qr-error-correction").addEventListener("change", () => {
  updateQrSettingsLabel();
  if (sender.file) loadFile(sender.file);
});
$("qr-fec-group").addEventListener("change", () => {
  updateQrSettingsLabel();
  if (sender.file) loadFile(sender.file);
});
$("qr-repeat-mode").addEventListener("change", () => {
  sender.repeatMode = $("qr-repeat-mode").value;
  updateQrSettingsLabel();
});
updateQrSettingsLabel();
window.addEventListener("beforeunload", () => { stopSender(); stopSenderControl(); stopReceiver(); });

try {
  await init();
  updateReceiverProgress();
  setStatus($("sender-file"), "WASM listo. Selecciona un archivo.");
  checkBackend();
} catch (error) {
  setStatus($("sender-file"), `No se pudo cargar WASM: ${error.message}`, true);
  $("start-receiver").disabled = true;
}
