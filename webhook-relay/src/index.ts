/**
 * Webhook Relay — Sidecar that connects to the Monad Execution Events Gateway
 * and forwards events to HTTP POST webhook endpoints.
 *
 * Core logic is NOT modified — this is a standalone process.
 *
 * Features:
 * - Connects to any gateway WebSocket channel
 * - Forwards each message as an HTTP POST with JSON body
 * - Exponential back-off retry per delivery (at-least-once)
 * - Auto-reconnect to the gateway with cursor resume
 * - Configurable via environment variables
 *
 * Environment variables:
 *   GATEWAY_URL     — Gateway WebSocket URL (default: ws://localhost:8443)
 *   GATEWAY_CHANNEL — Channel path (default: /v1/ws/lifecycle)
 *   WEBHOOK_URLS    — Comma-separated list of webhook target URLs
 *   MAX_RETRIES     — Max delivery retries per message (default: 5)
 *   RETRY_DELAY_MS  — Initial retry delay in ms (default: 500)
 *
 * Usage:
 *   GATEWAY_URL=ws://localhost:8443 \
 *   WEBHOOK_URLS=https://my-service.internal/hooks/blocks \
 *   node dist/index.js
 */

import WebSocket from "ws";

// ── Configuration ────────────────────────────────────────────────────

const GATEWAY_URL = process.env.GATEWAY_URL ?? "ws://localhost:8443";
const GATEWAY_CHANNEL = process.env.GATEWAY_CHANNEL ?? "/v1/ws/lifecycle";
const WEBHOOK_URLS = (process.env.WEBHOOK_URLS ?? "")
  .split(",")
  .map((u) => u.trim())
  .filter(Boolean);
const MAX_RETRIES = parseInt(process.env.MAX_RETRIES ?? "5", 10);
const RETRY_DELAY_MS = parseInt(process.env.RETRY_DELAY_MS ?? "500", 10);

if (WEBHOOK_URLS.length === 0) {
  console.error("WEBHOOK_URLS not set. Provide comma-separated target URLs.");
  process.exit(1);
}

// ── State ────────────────────────────────────────────────────────────

let lastSeqno: number | null = null;
let reconnectAttempts = 0;
let deliveredCount = 0;
let failedCount = 0;

// ── Delivery with retry ─────────────────────────────────────────────

async function deliverWithRetry(
  url: string,
  body: string,
  seqno: number
): Promise<boolean> {
  for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
    try {
      const res = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Gateway-Seqno": String(seqno),
        },
        body,
        signal: AbortSignal.timeout(10_000),
      });

      if (res.ok || (res.status >= 200 && res.status < 300)) {
        return true;
      }

      // 4xx = don't retry (client error, not transient)
      if (res.status >= 400 && res.status < 500) {
        console.error(
          `[delivery] ${url} returned ${res.status} — not retrying (seqno=${seqno})`
        );
        return false;
      }

      // 5xx = transient, retry
      console.warn(
        `[delivery] ${url} returned ${res.status} (attempt ${attempt + 1}/${MAX_RETRIES + 1}, seqno=${seqno})`
      );
    } catch (err) {
      console.warn(
        `[delivery] ${url} error (attempt ${attempt + 1}/${MAX_RETRIES + 1}, seqno=${seqno}):`,
        (err as Error).message
      );
    }

    if (attempt < MAX_RETRIES) {
      const delay = RETRY_DELAY_MS * Math.pow(2, attempt);
      await sleep(delay);
    }
  }

  return false;
}

async function fanOut(body: string, seqno: number): Promise<void> {
  const results = await Promise.allSettled(
    WEBHOOK_URLS.map((url) => deliverWithRetry(url, body, seqno))
  );

  for (const r of results) {
    if (r.status === "fulfilled" && r.value) {
      deliveredCount++;
    } else {
      failedCount++;
    }
  }
}

// ── WebSocket Connection ─────────────────────────────────────────────

function connectGateway(): void {
  let wsUrl = `${GATEWAY_URL.replace(/\/+$/, "")}${GATEWAY_CHANNEL}`;
  if (lastSeqno !== null) {
    const sep = wsUrl.includes("?") ? "&" : "?";
    wsUrl = `${wsUrl}${sep}resume_from=${lastSeqno}`;
  }

  console.log(`[relay] Connecting to ${wsUrl}`);
  console.log(`[relay] Forwarding to: ${WEBHOOK_URLS.join(", ")}`);

  const ws = new WebSocket(wsUrl);

  ws.on("open", () => {
    reconnectAttempts = 0;
    console.log("[relay] Connected to gateway");
  });

  ws.on("message", (data: WebSocket.Data) => {
    const raw = data.toString();

    try {
      const msg = JSON.parse(raw);
      const seqno: number = msg.server_seqno ?? 0;

      // Track cursor
      if (seqno > 0) {
        lastSeqno = seqno;
      }

      // Skip Resume control frames (not forwarded)
      if ("Resume" in msg) {
        const mode = msg.Resume?.mode ?? "unknown";
        console.log(`[relay] Resume ACK: mode=${mode}`);
        return;
      }

      // Forward everything else to webhooks
      fanOut(raw, seqno).catch((err) => {
        console.error("[relay] Fan-out error:", err);
      });
    } catch {
      // Ignore malformed
    }
  });

  ws.on("close", () => {
    console.log(
      `[relay] Disconnected (delivered=${deliveredCount}, failed=${failedCount})`
    );
    scheduleReconnect();
  });

  ws.on("error", (err) => {
    console.error("[relay] WebSocket error:", err.message);
  });
}

function scheduleReconnect(): void {
  reconnectAttempts++;
  const exp = Math.min(1000 * Math.pow(2, reconnectAttempts - 1), 30_000);
  const jitter = Math.random() * 500;
  const delay = Math.round(exp + jitter);

  console.log(
    `[relay] Reconnecting in ${delay}ms (attempt ${reconnectAttempts})`
  );
  setTimeout(connectGateway, delay);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ── Start ────────────────────────────────────────────────────────────

console.log("[relay] Monad Webhook Relay starting");
console.log(`[relay] Gateway: ${GATEWAY_URL}${GATEWAY_CHANNEL}`);
console.log(`[relay] Targets: ${WEBHOOK_URLS.length} webhook(s)`);
console.log(`[relay] Retry: max ${MAX_RETRIES} retries, ${RETRY_DELAY_MS}ms base delay`);
connectGateway();
