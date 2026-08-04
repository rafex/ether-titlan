import init, { prepare_binary_packets, prepare_fountain_packets } from "./pkg/qr_file_transfer.js";

let wasmReady;

async function prepare(request) {
  wasmReady ||= init();
  await wasmReady;
  const prepare = request.strategy === "fountain" ? prepare_fountain_packets : prepare_binary_packets;
  const option = request.strategy === "fountain" ? request.fountainOverhead : request.fecGroupSize;
  const packets = Array.from(prepare(
    new Uint8Array(request.buffer),
    request.filename,
    request.chunkBytes,
    option,
  ), (packet) => {
    const bytes = new Uint8Array(packet);
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  });
  self.postMessage({ id: request.id, packets }, packets);
}

self.addEventListener("message", (event) => {
  prepare(event.data).catch((error) => {
    self.postMessage({ id: event.data.id, error: error.message || String(error) });
  });
});
