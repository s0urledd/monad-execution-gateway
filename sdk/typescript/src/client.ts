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
 * Connects to a gateway WebSocket endpoint and emits typed events
 * for execution data, TPS metrics, and contention analytics.
 *
 * @example
 * ```ts
 * const client = new GatewayClient({ url: "ws://your-validator:8443" });
 *
 * client.on("event", (event) => {
 *   if (event.event_name === "BlockFinalized") {
 *     console.log("Block finalized:", event.payload);
 *   }
 * });
 *
 * client.on("tps", (tps) => console.log("TPS:", tps));
 *
 * await client.connect();
 *
 * // Filter to only block events and TPS
 * client.subscribe(["BlockStart", "BlockFinalized", "TPS"]);
 * ```
 */
export class GatewayClient {
  private ws: WebSocket | null = null;
  private options: Required<
    Pick<GatewayClientOptions, "url" | "autoReconnect" | "reconnectDelay" | "maxReconnectAttempts">
  > & { channel: Channel };
  private listeners: Map<string, Set<Function>> = new Map();
  private reconnectAttempts = 0;
  private shouldReconnect = true;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private baseUrl: string;
  private pendingSubscription: SimpleSubscribe | AdvancedSubscribe | null = null;

  constructor(options: GatewayClientOptions) {
    this.options = {
      autoReconnect: true,
      reconnectDelay: 3000,
      maxReconnectAttempts: Infinity,
      channel: "all",
      ...options,
    };

    this.baseUrl = this.options.url.replace(/\/+$/, "");
  }

  /**
   * Connect to the gateway WebSocket endpoint.
   * Resolves when the connection is established.
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

        // Re-send subscription after reconnect
        if (this.pendingSubscription) {
          this.sendRaw(JSON.stringify(this.pendingSubscription));
        }

        resolve();
      });

      this.ws.on("message", (data: WebSocket.Data) => {
        try {
          const msg: ServerMessage = JSON.parse(data.toString());
          this.handleMessage(msg);
        } catch {
          // ignore malformed messages
        }
      });

      this.ws.on("close", () => {
        this.emit("disconnected");
        this.maybeReconnect();
      });

      this.ws.on("error", (err: Error) => {
        this.emit("error", err);
        if (this.ws?.readyState !== WebSocket.OPEN) {
          reject(err);
        }
      });
    });
  }

  /**
   * Disconnect from the gateway. Stops auto-reconnect.
   */
  disconnect(): void {
    this.shouldReconnect = false;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  /**
   * Send a subscription message to filter events server-side.
   *
   * **Simple format** — subscribe to specific event types:
   * ```ts
   * client.subscribe(["BlockFinalized", "TPS", "ContentionData"]);
   * ```
   *
   * **Advanced format** — with field-level filters:
   * ```ts
   * client.subscribe({
   *   events: ["TxnLog"],
   *   filters: [{
   *     event_name: "TxnLog",
   *     field_filters: [
   *       { field: "address", filter: { values: ["0x..."] } }
   *     ]
   *   }]
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

  /**
   * Register an event listener.
   *
   * Available events:
   * - `"event"` — Individual execution event
   * - `"events"` — Batch of execution events
   * - `"tps"` — TPS update (2.5-block rolling window)
   * - `"contention"` — Per-block contention analytics
   * - `"topAccesses"` — Top accessed accounts and storage slots
   * - `"lifecycle"` — Block stage transition (Proposed/Voted/Finalized/Verified)
   * - `"connected"` / `"disconnected"` — Connection state
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
   * Remove an event listener.
   */
  off<K extends keyof GatewayClientEvents>(
    event: K,
    handler: EventHandler<K>
  ): this {
    this.listeners.get(event)?.delete(handler);
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

  /** Whether the client is currently connected */
  get connected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  // ─── REST Helpers ────────────────────────────────────────────────────

  /**
   * Fetch current TPS from the REST endpoint.
   *
   * @example
   * ```ts
   * const { tps } = await GatewayClient.fetchTPS("http://your-validator:8443");
   * ```
   */
  static async fetchTPS(baseUrl: string): Promise<TPSResponse> {
    const url = baseUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/tps`);
    return res.json() as Promise<TPSResponse>;
  }

  /**
   * Fetch latest contention data from the REST endpoint.
   *
   * @example
   * ```ts
   * const data = await GatewayClient.fetchContention("http://your-validator:8443");
   * ```
   */
  static async fetchContention(baseUrl: string): Promise<ContentionData> {
    const url = baseUrl.replace(/\/+$/, "");
    const res = await fetch(`${url}/v1/contention`);
    return res.json() as Promise<ContentionData>;
  }

  /**
   * Fetch gateway status from the REST endpoint.
   *
   * @example
   * ```ts
   * const status = await GatewayClient.fetchStatus("http://your-validator:8443");
   * console.log(status.block_number, status.connected_clients);
   * ```
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

  // ─── Private ──────────────────────────────────────────────────────

  private buildWsUrl(): string {
    const channelPath = CHANNEL_PATHS[this.options.channel];
    try {
      const parsed = new URL(this.baseUrl);
      if (parsed.pathname && parsed.pathname !== "/") {
        return this.baseUrl;
      }
    } catch {
      // not a valid URL, just append
    }
    return `${this.baseUrl}${channelPath}`;
  }

  private sendRaw(data: string): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(data);
    }
  }

  private handleMessage(msg: ServerMessage): void {
    if ("Events" in msg) {
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
  }

  private maybeReconnect(): void {
    if (
      !this.shouldReconnect ||
      !this.options.autoReconnect ||
      this.reconnectAttempts >= this.options.maxReconnectAttempts
    ) {
      return;
    }

    this.reconnectAttempts++;
    this.reconnectTimer = setTimeout(() => {
      this.connect().catch(() => {
        // reconnect failure handled by 'error' event
      });
    }, this.options.reconnectDelay);
  }
}
