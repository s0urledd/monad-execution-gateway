import WebSocket from "ws";
import type {
  GatewayClientOptions,
  GatewayClientEvents,
  ServerMessage,
  ExecEvent,
  EventName,
  ContentionData,
  TopAccessesData,
  BlockLifecycleUpdate,
  BlockLifecycleSummary,
  BlockStage,
  ResumeMode,
  Channel,
  SimpleSubscribe,
  AdvancedSubscribe,
  EventFilterSpec,
  TPSResponse,
  StatusResponse,
} from "./types";

type EventHandler<K extends keyof GatewayClientEvents> =
  GatewayClientEvents[K];

const CHANNEL_PATHS: Record<Channel, string> = {
  all: "/v1/ws",
  blocks: "/v1/ws/blocks",
  txs: "/v1/ws/txs",
  contention: "/v1/ws/contention",
  lifecycle: "/v1/ws/lifecycle",
};

/**
 * Client for the Monad Execution Events Gateway.
 *
 * Features:
 * - **Auto-reconnect** with exponential back-off and jitter
 * - **Cursor resume** — tracks `server_seqno`, reconnects with `?resume_from`
 * - **Resume ACK validation** — verifies the server's Resume control frame
 * - **Heartbeat detection** — reconnects when no data arrives within timeout
 * - **Channel abstraction** — lifecycle / raw / contention / blocks / txs
 * - **Typed events** — strongly typed payloads for every event kind
 *
 * @example
 * ```ts
 * const client = new GatewayClient({
 *   url: "wss://gateway.monad.xyz",
 *   channel: "lifecycle",
 *   heartbeatTimeout: 10_000,
 * });
 *
 * client.on("lifecycle", (update) => {
 *   if (update.to_stage === "Finalized") {
 *     console.log(`Block ${update.block_number} finalized in ${update.block_age_ms}ms`);
 *   }
 * });
 *
 * await client.connect();
 * ```
 */
export class GatewayClient {
  private ws: WebSocket | null = null;
  private options: Required<
    Pick<
      GatewayClientOptions,
      | "url"
      | "autoReconnect"
      | "reconnectDelay"
      | "maxReconnectDelay"
      | "maxReconnectAttempts"
      | "heartbeatTimeout"
    >
  > & { channel: Channel };
  private listeners: Map<string, Set<Function>> = new Map();
  private onceListeners: Map<string, Set<Function>> = new Map();
  private reconnectAttempts = 0;
  private shouldReconnect = true;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
  private baseUrl: string;
  private pendingSubscription: SimpleSubscribe | AdvancedSubscribe | null = null;

  /**
   * Tracks the last `server_seqno` received from the gateway.
   * On reconnect this is sent as `?resume_from=<seqno>` so the server
   * replays only the messages the client missed.
   */
  private lastServerSeqno: number | null = null;

  constructor(options: GatewayClientOptions) {
    this.options = {
      autoReconnect: true,
      reconnectDelay: 1_000,
      maxReconnectDelay: 30_000,
      maxReconnectAttempts: Infinity,
      heartbeatTimeout: 10_000,
      channel: "all",
      ...options,
    };

    this.baseUrl = this.options.url.replace(/\/+$/, "");
  }

  // ─── Connection ────────────────────────────────────────────────────

  /**
   * Connect to the gateway WebSocket endpoint.
   * Resolves when the connection is established and the Resume ACK is received.
   */
  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.shouldReconnect = true;

      const wsUrl = this.buildWsUrl();

      try {
        this.ws = new WebSocket(wsUrl);
      } catch (err) {
        reject(err);
        return;
      }

      this.ws.on("open", () => {
        this.reconnectAttempts = 0;
        this.emit("connected");
        this.resetHeartbeat();

        // Re-send subscription after reconnect
        if (this.pendingSubscription) {
          this.sendRaw(JSON.stringify(this.pendingSubscription));
        }

        resolve();
      });

      this.ws.on("message", (data: WebSocket.Data) => {
        this.resetHeartbeat();
        try {
          const msg: ServerMessage = JSON.parse(data.toString());
          this.handleMessage(msg);
        } catch {
          // ignore malformed messages
        }
      });

      this.ws.on("close", () => {
        this.clearHeartbeat();
        this.emit("disconnected");
        this.maybeReconnect();
      });

      this.ws.on("error", (err: Error) => {
        this.emit("error", err);
        if (this.ws?.readyState !== WebSocket.OPEN) {
          reject(err);
        }
      });

      // WebSocket pong frames also count as heartbeat
      this.ws.on("pong", () => {
        this.resetHeartbeat();
      });
    });
  }

  /**
   * Disconnect from the gateway. Stops auto-reconnect.
   */
  disconnect(): void {
    this.shouldReconnect = false;
    this.clearHeartbeat();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  // ─── Subscriptions ─────────────────────────────────────────────────

  /**
   * Send a subscription message to filter events server-side.
   *
   * **Simple format** — subscribe to specific event types:
   * ```ts
   * client.subscribe(["BlockFinalized", "TPS", "ContentionData"]);
   * ```
   *
   * **Advanced format** — with field-level filters and stage gating:
   * ```ts
   * client.subscribe({
   *   events: ["TxnLog"],
   *   filters: [{
   *     event_name: "TxnLog",
   *     field_filters: [
   *       { field: "address", filter: { values: ["0x..."] } }
   *     ]
   *   }],
   *   min_stage: "Finalized",
   * });
   * ```
   */
  subscribe(
    sub: string[] | { events: string[]; filters?: EventFilterSpec[]; min_stage?: BlockStage }
  ): void {
    let msg: SimpleSubscribe | AdvancedSubscribe;

    if (Array.isArray(sub)) {
      msg = { subscribe: sub };
    } else {
      msg = { subscribe: sub };
    }

    this.pendingSubscription = msg;

    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.sendRaw(JSON.stringify(msg));
    }
  }

  // ─── Event Emitter ─────────────────────────────────────────────────

  /**
   * Register an event listener.
   *
   * Available events:
   * - `"event"` — Individual execution event
   * - `"events"` — Batch of execution events
   * - `"tps"` — TPS update (2.5-block rolling window)
   * - `"contention"` — Per-block contention analytics
   * - `"topAccesses"` — Top accessed accounts and storage slots
   * - `"lifecycle"` — Block stage transition
   * - `"resume"` — Resume ACK from server (first frame)
   * - `"connected"` / `"disconnected"` / `"reconnecting"` — Connection state
   * - `"error"` — WebSocket errors
   */
  on<K extends keyof GatewayClientEvents>(
    event: K,
    handler: EventHandler<K>
  ): this {
    if (!this.listeners.has(event)) {
      this.listeners.set(event, new Set());
    }
    this.listeners.get(event)!.add(handler);
    return this;
  }

  /**
   * Register a one-shot event listener. Automatically removed after first call.
   */
  once<K extends keyof GatewayClientEvents>(
    event: K,
    handler: EventHandler<K>
  ): this {
    if (!this.onceListeners.has(event)) {
      this.onceListeners.set(event, new Set());
    }
    this.onceListeners.get(event)!.add(handler);
    return this;
  }

  /**
   * Remove an event listener.
   */
  off<K extends keyof GatewayClientEvents>(
    event: K,
    handler: EventHandler<K>
  ): this {
    this.listeners.get(event)?.delete(handler);
    this.onceListeners.get(event)?.delete(handler);
    return this;
  }

  /**
   * Register a listener for a specific execution event type.
   *
   * @example
   * ```ts
   * client.onEvent("BlockFinalized", (event) => {
   *   console.log("Block:", event.block_number);
   * });
   * ```
   */
  onEvent(eventName: EventName, handler: (event: ExecEvent) => void): this {
    return this.on("event", (event) => {
      if (event.event_name === eventName) {
        handler(event);
      }
    });
  }

  // ─── Getters ───────────────────────────────────────────────────────

  /** Whether the client is currently connected */
  get connected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  /** Last server_seqno received — pass to `?resume_from` for cursor resume */
  get serverSeqno(): number | null {
    return this.lastServerSeqno;
  }

  /**
   * Wait for the Resume ACK from the server.
   * Returns the resume mode (`"resume"` = lossless replay, `"snapshot"` = state snapshot).
   *
   * @example
   * ```ts
   * await client.connect();
   * const { mode } = await client.waitForResume(5000);
   * if (mode === "snapshot") {
   *   console.log("Gap detected — rebuilding state from snapshot");
   * }
   * ```
   */
  waitForResume(timeoutMs = 5000): Promise<ResumeMode> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error("Resume ACK timeout"));
      }, timeoutMs);

      this.once("resume", ((info: ResumeMode) => {
        clearTimeout(timer);
        resolve(info);
      }) as EventHandler<"resume">);
    });
  }

  // ─── REST Helpers ──────────────────────────────────────────────────

  /**
   * Fetch current TPS from the REST endpoint.
   */
  static async fetchTPS(baseUrl: string): Promise<TPSResponse> {
    const url = baseUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/tps`);
    return res.json() as Promise<TPSResponse>;
  }

  /**
   * Fetch latest contention data from the REST endpoint.
   */
  static async fetchContention(baseUrl: string): Promise<ContentionData> {
    const url = baseUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/contention`);
    return res.json() as Promise<ContentionData>;
  }

  /**
   * Fetch gateway status from the REST endpoint.
   */
  static async fetchStatus(baseUrl: string): Promise<StatusResponse> {
    const url = baseUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/status`);
    return res.json() as Promise<StatusResponse>;
  }

  /**
   * Fetch lifecycle summaries for all recent blocks.
   */
  static async fetchLifecycle(baseUrl: string): Promise<BlockLifecycleSummary[]> {
    const url = baseUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/blocks/lifecycle`);
    return res.json() as Promise<BlockLifecycleSummary[]>;
  }

  /**
   * Fetch the full lifecycle for a specific block.
   */
  static async fetchBlockLifecycle(baseUrl: string, blockNumber: number): Promise<BlockLifecycleSummary> {
    const url = baseUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/blocks/${blockNumber}/lifecycle`);
    return res.json() as Promise<BlockLifecycleSummary>;
  }

  // ─── Private ───────────────────────────────────────────────────────

  private buildWsUrl(): string {
    const channelPath = CHANNEL_PATHS[this.options.channel];
    let wsUrl: string;
    try {
      const parsed = new URL(this.baseUrl);
      if (parsed.pathname && parsed.pathname !== "/") {
        wsUrl = this.baseUrl;
      } else {
        wsUrl = `${this.baseUrl}${channelPath}`;
      }
    } catch {
      wsUrl = `${this.baseUrl}${channelPath}`;
    }

    // Append resume_from cursor for seamless reconnect
    if (this.lastServerSeqno !== null) {
      const separator = wsUrl.includes("?") ? "&" : "?";
      wsUrl = `${wsUrl}${separator}resume_from=${this.lastServerSeqno}`;
    }

    return wsUrl;
  }

  private sendRaw(data: string): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(data);
    }
  }

  private handleMessage(msg: ServerMessage): void {
    // Track cursor position for resume-on-reconnect
    if (msg.server_seqno !== undefined) {
      this.lastServerSeqno = msg.server_seqno;
    }

    if ("Resume" in msg) {
      this.emit("resume", msg.Resume);
    } else if ("Events" in msg) {
      this.emit("events", msg.Events);
      for (const event of msg.Events) {
        this.emit("event", event);
      }
    } else if ("TPS" in msg) {
      this.emit("tps", msg.TPS);
    } else if ("ContentionData" in msg) {
      this.emit("contention", msg.ContentionData);
    } else if ("TopAccesses" in msg) {
      this.emit("topAccesses", msg.TopAccesses);
    } else if ("Lifecycle" in msg) {
      this.emit("lifecycle", msg.Lifecycle);
    }
  }

  private emit(event: string, ...args: unknown[]): void {
    const handlers = this.listeners.get(event);
    if (handlers) {
      for (const handler of handlers) {
        try {
          handler(...args);
        } catch {
          // don't let user handler errors kill the client
        }
      }
    }

    // Fire-and-remove once listeners
    const onceHandlers = this.onceListeners.get(event);
    if (onceHandlers && onceHandlers.size > 0) {
      for (const handler of onceHandlers) {
        try {
          handler(...args);
        } catch {
          // same guard
        }
      }
      onceHandlers.clear();
    }
  }

  // ─── Heartbeat ─────────────────────────────────────────────────────

  private resetHeartbeat(): void {
    this.clearHeartbeat();
    const timeout = this.options.heartbeatTimeout;
    if (timeout <= 0) return;

    this.heartbeatTimer = setTimeout(() => {
      // No data received within timeout — assume dead connection
      this.emit("error", new Error(`No data received for ${timeout}ms — reconnecting`));
      if (this.ws) {
        this.ws.terminate();  // hard-close, triggers 'close' → maybeReconnect
      }
    }, timeout);
  }

  private clearHeartbeat(): void {
    if (this.heartbeatTimer) {
      clearTimeout(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  // ─── Reconnect with Exponential Back-off ───────────────────────────

  private maybeReconnect(): void {
    if (
      !this.shouldReconnect ||
      !this.options.autoReconnect ||
      this.reconnectAttempts >= this.options.maxReconnectAttempts
    ) {
      return;
    }

    this.reconnectAttempts++;

    // Exponential back-off with jitter: delay * 2^(attempt-1) + random jitter
    const exponential = Math.min(
      this.options.reconnectDelay * Math.pow(2, this.reconnectAttempts - 1),
      this.options.maxReconnectDelay
    );
    const jitter = Math.random() * this.options.reconnectDelay * 0.5;
    const delay = Math.round(exponential + jitter);

    this.emit("reconnecting", this.reconnectAttempts, delay);

    this.reconnectTimer = setTimeout(() => {
      this.connect().catch(() => {
        // reconnect failure handled by 'error' event → will trigger another maybeReconnect
      });
    }, delay);
  }
}
