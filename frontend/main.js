import init, {
  prepare_file,
  process_packet,
  receiver_filename,
  receiver_progress,
  reset_receiver,
} from "./pkg/qr_file_transfer.js";

const MAX_FILE_BYTES = 1.5 * 1024 * 1024;
const QR_SIZE = 640;
const PACKET_HOLD_FRAMES = 3;
const MAX_SCAN_WIDTH = 1280;
const QR_OPTIONS = {
  errorCorrectionLevel: "M",
  margin: 4,
  width: QR_SIZE,
  color: { dark: "#000000", light: "#ffffff" },
};

const $ = (id) => document.getElementById(id);
const sender = {
  packets: [],
  file: null,
  frame: null,
  index: 0,
  rendering: false,
  running: false,
  packetRendered: false,
  packetFrames: 0,
};
const receiver = { stream: null, frame: null, running: false, lastPacket: "" };

const senderCanvas = $("sender-canvas");
const scanCanvas = $("scan-canvas");
const scanContext = scanCanvas.getContext("2d", { willReadFrequently: true });

function setStatus(element, message, isError = false) {
  element.textContent = message;
  element.classList.toggle("error", isError);
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
  const total = sender.packets.length;
  const current = total ? sender.index + 1 : 0;
  $("sender-progress").max = Math.max(total, 1);
  $("sender-progress").value = current;
  $("sender-progress-label").textContent = `Paquete ${current} de ${total}`;
}

function updateReceiverProgress() {
  const [received, total] = receiver_progress();
  $("receiver-progress").max = Math.max(total, 1);
  $("receiver-progress").value = received;
  $("receiver-progress-label").textContent = `Paquetes recibidos: ${received} de ${total}`;
}

function stopSender() {
  sender.running = false;
  sender.frame = null;
  sender.rendering = false;
  sender.packetRendered = false;
  sender.packetFrames = 0;
  $("start-sender").disabled = !sender.packets.length;
  $("stop-sender").disabled = true;
}

function renderSenderFrame() {
  if (!sender.running) return;
  sender.frame = requestAnimationFrame(renderSenderFrame);
  if (sender.rendering || !sender.packets.length || !window.QRCode) return;

  if (sender.packetRendered) {
    sender.packetFrames += 1;
    if (sender.packetFrames < PACKET_HOLD_FRAMES) return;
    sender.packetRendered = false;
    sender.packetFrames = 0;
    sender.index = (sender.index + 1) % sender.packets.length;
    updateSenderProgress();
  }

  const payload = sender.packets[sender.index];
  sender.rendering = true;
  window.QRCode.toCanvas(senderCanvas, payload, QR_OPTIONS, (error) => {
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
  sender.running = true;
  sender.index = 0;
  sender.packetRendered = false;
  sender.packetFrames = 0;
  $("start-sender").disabled = true;
  $("stop-sender").disabled = false;
  setStatus($("sender-file"), `${sender.file.name} — emisión activa, QR repetido para captura móvil.`);
  renderSenderFrame();
}

async function loadFile(file) {
  stopSender();
  sender.packets = [];
  sender.file = null;
  updateSenderProgress();

  if (!file) return;
  if (file.size > MAX_FILE_BYTES) {
    setStatus($("sender-file"), "El archivo supera el límite de 1.5 MiB.", true);
    return;
  }

  try {
    const buffer = await file.arrayBuffer();
    sender.packets = prepare_file(new Uint8Array(buffer), file.name);
    if (!sender.packets.length) throw new Error("WASM rechazó el archivo o el nombre es demasiado largo.");
    sender.file = file;
    $("start-sender").disabled = false;
    setStatus($("sender-file"), `${file.name} — ${(file.size / 1024).toFixed(1)} KiB listo.`);
    updateSenderProgress();
  } catch (error) {
    setStatus($("sender-file"), `No se pudo preparar el archivo: ${error.message}`, true);
  }
}

function stopReceiver() {
  receiver.running = false;
  if (receiver.frame) cancelAnimationFrame(receiver.frame);
  receiver.frame = null;
  if (receiver.stream) receiver.stream.getTracks().forEach((track) => track.stop());
  receiver.stream = null;
  $("receiver-video").srcObject = null;
  $("start-receiver").disabled = false;
  $("stop-receiver").disabled = true;
}

function finishDownload(bytes) {
  const filename = receiver_filename() || "archivo-recibido.bin";
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
}

function scanReceiverFrame() {
  if (!receiver.running) return;
  receiver.frame = requestAnimationFrame(scanReceiverFrame);
  const video = $("receiver-video");
  if (video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA || !video.videoWidth) return;

  const scale = Math.min(1, MAX_SCAN_WIDTH / video.videoWidth);
  const width = Math.max(640, Math.round(video.videoWidth * scale));
  const height = Math.round(video.videoHeight * (width / video.videoWidth));
  if (scanCanvas.width !== width || scanCanvas.height !== height) {
    scanCanvas.width = width;
    scanCanvas.height = height;
  }
  scanContext.imageSmoothingEnabled = false;
  scanContext.drawImage(video, 0, 0, width, height);
  const image = scanContext.getImageData(0, 0, width, height);
  const code = window.jsQR?.(image.data, image.width, image.height, { inversionAttempts: "attemptBoth" });
  if (!code?.data || code.data === receiver.lastPacket) return;

  receiver.lastPacket = code.data;
  try {
    const assembled = process_packet(code.data);
    updateReceiverProgress();
    if (assembled !== null && assembled !== undefined) finishDownload(assembled);
  } catch (error) {
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
    reset_receiver();
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
    receiver.running = true;
    $("start-receiver").disabled = true;
    $("stop-receiver").disabled = false;
    setStatus($("receiver-status"), "Cámara activa. Buscando paquetes…");
    scanReceiverFrame();
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
$("start-receiver").addEventListener("click", startReceiver);
$("stop-receiver").addEventListener("click", stopReceiver);
document.querySelectorAll(".tab").forEach((button) => button.addEventListener("click", () => switchTab(button.dataset.tab)));
window.addEventListener("beforeunload", () => { stopSender(); stopReceiver(); });

try {
  await init();
  setStatus($("sender-file"), "WASM listo. Selecciona un archivo.");
  checkBackend();
} catch (error) {
  setStatus($("sender-file"), `No se pudo cargar WASM: ${error.message}`, true);
  $("start-receiver").disabled = true;
}
