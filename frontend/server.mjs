import { createReadStream } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { request as httpRequest } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import path from "node:path";
import { fileURLToPath } from "node:url";

const frontendRoot = path.dirname(fileURLToPath(import.meta.url));
const port = Number(process.env.PORT || 8443);
const backendUrl = new URL(process.env.BACKEND_URL || "http://127.0.0.1:9000");
const tlsCert = process.env.TLS_CERT || "/tmp/qr-transfer-cert/server.crt";
const tlsKey = process.env.TLS_KEY || "/tmp/qr-transfer-cert/server.key";

const contentTypes = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
  ".png": "image/png",
};

function sendJson(response, statusCode, body) {
  const payload = JSON.stringify(body);
  response.writeHead(statusCode, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(payload),
  });
  response.end(payload);
}

function proxyToBackend(request, response) {
  const target = new URL(request.url, backendUrl);
  const proxy = httpRequest(
    {
      hostname: target.hostname,
      port: target.port,
      path: `${target.pathname}${target.search}`,
      method: request.method,
      headers: { ...request.headers, host: target.host },
    },
    (backendResponse) => {
      response.writeHead(backendResponse.statusCode || 502, backendResponse.headers);
      backendResponse.pipe(response);
    },
  );
  proxy.on("error", (error) => {
    if (!response.headersSent) sendJson(response, 502, { error: "backend_unavailable", detail: error.message });
    else response.destroy(error);
  });
  request.pipe(proxy);
}

async function serveStatic(request, response) {
  let requestPath;
  try {
    requestPath = decodeURIComponent(new URL(request.url, "https://frontend.local").pathname);
  } catch {
    sendJson(response, 400, { error: "invalid_path" });
    return;
  }
  const relativePath = requestPath === "/" ? "index.html" : requestPath.slice(1);
  const filePath = path.resolve(frontendRoot, relativePath);
  const rootPrefix = `${frontendRoot}${path.sep}`;

  if (filePath !== frontendRoot && !filePath.startsWith(rootPrefix)) {
    sendJson(response, 403, { error: "forbidden" });
    return;
  }

  try {
    const fileInfo = await stat(filePath);
    if (!fileInfo.isFile()) throw new Error("not a file");
    response.writeHead(200, {
      "content-type": contentTypes[path.extname(filePath)] || "application/octet-stream",
      "content-length": fileInfo.size,
      "cache-control": "no-store",
    });
    if (request.method === "HEAD") response.end();
    else createReadStream(filePath).pipe(response);
  } catch {
    sendJson(response, 404, { error: "not_found" });
  }
}

async function handleRequest(request, response) {
  if (request.url?.startsWith("/api/")) {
    proxyToBackend(request, response);
    return;
  }
  if (request.method !== "GET" && request.method !== "HEAD") {
    sendJson(response, 405, { error: "method_not_allowed" });
    return;
  }
  await serveStatic(request, response);
}

const [cert, key] = await Promise.all([readFile(tlsCert), readFile(tlsKey)]);
const server = createHttpsServer({ cert, key }, handleRequest);

server.listen(port, "0.0.0.0", () => {
  console.log(`Frontend HTTPS server listening on 0.0.0.0:${port}`);
  console.log(`Backend proxy target: ${backendUrl.origin}`);
});
