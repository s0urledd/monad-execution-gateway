/**
 * Monad Execution Events — Type Definitions
 *
 * These types map 1:1 to the gateway's JSON wire format.
 * All hex values (addresses, hashes, U256) are 0x-prefixed strings.
 */

// ─── Event Names ────────────────────────────────────────────────────

export type EventName =
  | "RecordError"
  | "BlockStart"
  | "BlockReject"
  | "BlockPerfEvmEnter"
  | "BlockPerfEvmExit"
  | "BlockEnd"
  | "BlockQC"
  | "BlockFinalized"
  | "BlockVerified"
  | "TxnHeaderStart"
  | "TxnAccessListEntry"
  | "TxnAuthListEntry"
  | "TxnHeaderEnd"
  | "TxnReject"
  | "TxnPerfEvmEnter"
  | "TxnPerfEvmExit"
  | "TxnEvmOutput"
  | "TxnLog"
  | "TxnCallFrame"
  | "TxnEnd"
  | "AccountAccessListHeader"
  | "AccountAccess"
  | "StorageAccess"
  | "EvmError"
  | "NativeTransfer";

// ─── Event Payloads ─────────────────────────────────────────────────

export interface BlockStartPayload {
  type: "BlockStart";
  block_number: number;
  block_id: string;
  round: number;
  epoch: number;
  parent_eth_hash: string;
  timestamp: number;
  beneficiary: string;
  gas_limit: number;
  base_fee_per_gas: string;
}

export interface BlockEndPayload {
  type: "BlockEnd";
  eth_block_hash: string;
  state_root: string;
  receipts_root: string;
  logs_bloom: string;
  gas_used: number;
}

export interface BlockQCPayload {
  type: "BlockQC";
  block_id: string;
  block_number: number;
  round: number;
}

export interface BlockFinalizedPayload {
  type: "BlockFinalized";
  block_id: string;
  block_number: number;
}

export interface BlockVerifiedPayload {
  type: "BlockVerified";
  block_number: number;
}

export interface BlockRejectPayload {
  type: "BlockReject";
  reason: number;
}

export interface TxnHeaderStartPayload {
  type: "TxnHeaderStart";
  txn_index: number;
  txn_hash: string;
  sender: string;
  txn_type: number;
  chain_id: string;
  nonce: number;
  gas_limit: number;
  max_fee_per_gas: string;
  max_priority_fee_per_gas: string;
  value: string;
  data: string;
  to: string;
  is_contract_creation: boolean;
  r: string;
  s: string;
  y_parity: boolean;
  access_list_count: number;
  auth_list_count: number;
}

export interface TxnEvmOutputPayload {
  type: "TxnEvmOutput";
  txn_index: number;
  log_count: number;
  status: boolean;
  gas_used: number;
}

export interface TxnLogPayload {
  type: "TxnLog";
  txn_index: number;
  log_index: number;
  address: string;
  topics: string;
  data: string;
}

export interface TxnCallFramePayload {
  type: "TxnCallFrame";
  txn_index: number;
  depth: number;
  caller: string;
  call_target: string;
  value: string;
  input: string;
  output: string;
}

export interface TxnRejectPayload {
  type: "TxnReject";
  txn_index: number;
  reason: number;
}

export interface TxnAccessListEntryPayload {
  type: "TxnAccessListEntry";
  txn_index: number;
  address: string;
  storage_keys: string;
}

export interface TxnAuthListEntryPayload {
  type: "TxnAuthListEntry";
  txn_index: number;
  address: string;
}

export interface AccountAccessListHeaderPayload {
  type: "AccountAccessListHeader";
  txn_index: number | null;
  entry_count: number;
}

export interface AccountAccessPayload {
  type: "AccountAccess";
  txn_index: number | null;
  address: string;
  balance: string;
  nonce: number;
  code_hash: string;
}

export interface StorageAccessPayload {
  type: "StorageAccess";
  txn_index: number | null;
  account_index: number;
  key: string;
  value: string;
}

export interface RecordErrorPayload {
  type: "RecordError";
  error_type: number;
  dropped_event_type: number;
  truncated_payload_size: number;
  requested_payload_size: number;
}

export interface EvmErrorPayload {
  type: "EvmError";
  domain_id: number;
  status_code: number;
}

export interface EmptyPayload {
  type:
    | "BlockPerfEvmEnter"
    | "BlockPerfEvmExit"
    | "TxnHeaderEnd"
    | "TxnPerfEvmEnter"
    | "TxnPerfEvmExit"
    | "TxnEnd";
}

export type ExecEventPayload =
  | BlockStartPayload
  | BlockEndPayload
  | BlockQCPayload
  | BlockFinalizedPayload
  | BlockVerifiedPayload
  | BlockRejectPayload
  | TxnHeaderStartPayload
  | TxnEvmOutputPayload
  | TxnLogPayload
  | TxnCallFramePayload
  | TxnRejectPayload
  | TxnAccessListEntryPayload
  | TxnAuthListEntryPayload
  | AccountAccessListHeaderPayload
  | AccountAccessPayload
  | StorageAccessPayload
  | RecordErrorPayload
  | EvmErrorPayload
  | EmptyPayload;

// ─── Block Commit Stages ────────────────────────────────────────────

/**
 * Public block commit stages aligned with Monad's MonadBFT consensus.
 *
 * Happy path: Proposed → Voted → Finalized → Verified
 *
 * Timing:
 *   Proposed  → block proposed for execution
 *   Voted     → quorum certificate received (~400ms, speculative finality)
 *   Finalized → committed to canonical chain (~800ms, full finality)
 *   Verified  → state root verified (terminal)
 *   Rejected  → dropped at any point (terminal)
 */
export type BlockStage =
  | "Proposed"
  | "Voted"
  | "Finalized"
  | "Verified"
  | "Rejected";

/**
 * Emitted on every block stage transition via the /v1/ws/lifecycle channel
 * and also included in the "all" and "blocks" channels.
 */
export interface BlockLifecycleUpdate {
  block_hash: string;
  block_number: number;
  from_stage?: BlockStage;
  to_stage: BlockStage;
  /** Milliseconds spent in the previous stage */
  time_in_previous_stage_ms?: number;
  /** Total elapsed time since Proposed (ms) */
  block_age_ms: number;
  txn_count: number;
  gas_used?: number;
}

/**
 * Full lifecycle summary for a single block (REST response).
 */
export interface BlockLifecycleSummary {
  block_hash: string;
  block_number: number;
  current_stage: BlockStage;
  txn_count: number;
  gas_used?: number;
  eth_block_hash?: string;
  /** Milliseconds from Proposed to each stage reached */
  stage_timings_ms: Partial<Record<BlockStage, number>>;
  execution_time_ms?: number;
  total_age_ms?: number;
}

// ─── Event Wrapper ──────────────────────────────────────────────────

export interface ExecEvent {
  event_name: EventName;
  block_number?: number;
  txn_idx?: number;
  txn_hash?: string;
  /** Current public commit stage of this event's block */
  commit_stage?: BlockStage;
  payload: ExecEventPayload;
  seqno: number;
  timestamp_ns: number;
}

// ─── Computed Metrics ───────────────────────────────────────────────

export interface AccessEntry<T> {
  key: T;
  count: number;
}

export interface TopAccessesData {
  account: AccessEntry<string>[];
  storage: AccessEntry<[string, string]>[];
}

export interface ContendedSlotEntry {
  address: string;
  slot: string;
  txn_count: number;
  access_count: number;
}

export interface ContractContentionEntry {
  address: string;
  total_slots: number;
  contended_slots: number;
  total_accesses: number;
  contention_score: number;
}

export interface ContractEdge {
  contract_a: string;
  contract_b: string;
  shared_txn_count: number;
}

export interface ContentionData {
  block_number: number;
  block_wall_time_ns: number;
  total_tx_time_ns: number;
  parallel_efficiency_pct: number;
  total_unique_slots: number;
  contended_slot_count: number;
  contention_ratio: number;
  total_txn_count: number;
  top_contended_slots: ContendedSlotEntry[];
  top_contended_contracts: ContractContentionEntry[];
  contract_edges: ContractEdge[];
}

// ─── Server Messages ────────────────────────────────────────────────

/**
 * Resume control message sent as the first frame after connect/reconnect.
 * `mode` is `"resume"` when the cursor was valid (buffered replay) or
 * `"snapshot"` when the cursor was too old or on fresh connect.
 */
export interface ResumeMode {
  mode: "resume" | "snapshot";
}

/**
 * Every message from the server carries a `server_seqno` — a monotonic
 * counter that uniquely identifies each wire message.  On reconnect the
 * client can pass `?resume_from=<server_seqno>` to pick up exactly
 * where it left off without re-receiving already-processed data.
 */
export type ServerMessage = {
  server_seqno: number;
} & (
  | { Events: ExecEvent[] }
  | { TopAccesses: TopAccessesData }
  | { TPS: number }
  | { ContentionData: ContentionData }
  | { Lifecycle: BlockLifecycleUpdate }
  | { Resume: ResumeMode }
);

// ─── Channels ───────────────────────────────────────────────────────

export type Channel = "all" | "blocks" | "txs" | "contention" | "lifecycle";

// ─── Subscription Protocol ──────────────────────────────────────────

/** Simple subscribe: list of event names and/or metric types */
export interface SimpleSubscribe {
  subscribe: string[];
}

/**
 * Field-level filter for advanced subscriptions.
 * Maps to the server's FieldFilter enum (serde tag="field", content="filter").
 *
 * Supported fields:
 * - `txn_index`: Range filter `{ min?: number, max?: number }`
 * - `log_index`: Range filter `{ min?: number, max?: number }`
 * - `address`: Exact match `{ values: ["0x..."] }`
 * - `topics`: Array prefix match `{ values: ["0x..."] }`
 */
export interface FieldFilter {
  field: "txn_index" | "log_index" | "address" | "topics";
  filter: {
    values?: string[];
    min?: number;
    max?: number;
  };
}

/**
 * Filter spec for a single event type with optional field filters.
 * Multiple field filters are combined with AND logic.
 */
export interface EventFilterSpec {
  event_name: EventName;
  field_filters?: FieldFilter[];
}

/**
 * Advanced subscribe: events + field-level filters.
 *
 * The `filters` array uses EventFilterSpec format matching the server's
 * Rust `EventFilterSpec` struct. Multiple specs are combined with OR logic.
 *
 * @example
 * ```ts
 * client.subscribe({
 *   events: ["TxnLog"],
 *   filters: [{
 *     event_name: "TxnLog",
 *     field_filters: [
 *       { field: "address", filter: { values: ["0xabc..."] } }
 *     ]
 *   }]
 * });
 * ```
 */
export interface AdvancedSubscribe {
  subscribe: {
    events: string[];
    filters?: EventFilterSpec[];
    /** Only deliver events from blocks at or above this commit stage */
    min_stage?: BlockStage;
  };
}

export type SubscribeMessage = SimpleSubscribe | AdvancedSubscribe;

// ─── REST Responses ─────────────────────────────────────────────────

export interface TPSResponse {
  tps: number;
}

export interface StatusResponse {
  status: "healthy" | "degraded";
  block_number: number;
  connected_clients: number;
  uptime_secs: number;
  last_event_age_secs: number;
  /** Current server-level sequence number (for cursor resume) */
  server_seqno: number;
  /** Oldest seqno still in the ring buffer (0 = buffer empty) */
  oldest_seqno: number;
  /** Newest seqno assigned so far (same as server_seqno) */
  newest_seqno: number;
}

export interface LifecycleResponse extends Array<BlockLifecycleSummary> {}

// ─── Client Options ─────────────────────────────────────────────────

export interface GatewayClientOptions {
  /** WebSocket URL of the gateway (e.g. "ws://your-validator:8443") */
  url: string;
  /** WebSocket channel to connect to (default: "all") */
  channel?: Channel;
  /** Auto-reconnect on disconnect (default: true) */
  autoReconnect?: boolean;
  /** Initial reconnect delay in ms — doubles each attempt (default: 1000) */
  reconnectDelay?: number;
  /** Cap for exponential backoff in ms (default: 30000) */
  maxReconnectDelay?: number;
  /** Maximum reconnect attempts (default: Infinity) */
  maxReconnectAttempts?: number;
  /**
   * Heartbeat timeout in ms.  If no message arrives within this window
   * the client considers the connection dead and triggers a reconnect.
   * Set to 0 to disable. (default: 10000)
   */
  heartbeatTimeout?: number;
}

// ─── Client Events ──────────────────────────────────────────────────

export interface GatewayClientEvents {
  event: (event: ExecEvent) => void;
  events: (events: ExecEvent[]) => void;
  tps: (tps: number) => void;
  contention: (data: ContentionData) => void;
  topAccesses: (data: TopAccessesData) => void;
  lifecycle: (update: BlockLifecycleUpdate) => void;
  /** Emitted as the first frame after connect/reconnect with the resume mode */
  resume: (info: ResumeMode) => void;
  connected: () => void;
  disconnected: () => void;
  reconnecting: (attempt: number, delayMs: number) => void;
  error: (error: Error) => void;
}
