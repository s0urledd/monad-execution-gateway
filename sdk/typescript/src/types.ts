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

// ─── Event Wrapper ──────────────────────────────────────────────────

export interface ExecEvent {
  event_name: EventName;
  block_number?: number;
  txn_idx?: number;
  txn_hash?: string;
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

export type ServerMessage =
  | { Events: ExecEvent[] }
  | { TopAccesses: TopAccessesData }
  | { TPS: number }
  | { ContentionData: ContentionData };

// ─── Channels ───────────────────────────────────────────────────────

export type Channel = "all" | "blocks" | "txs" | "contention";

// ─── Subscription Protocol ──────────────────────────────────────────

/** Simple subscribe: list of event names and/or metric types */
export interface SimpleSubscribe {
  subscribe: string[];
}

/** Advanced subscribe: events + field-level filters */
export interface AdvancedSubscribe {
  subscribe: {
    events: string[];
    filters?: Array<{
      event_name: EventName;
      field_name: string;
      field_value: string;
    }>;
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
}

// ─── Client Options ─────────────────────────────────────────────────

export interface GatewayClientOptions {
  /** WebSocket URL of the gateway (e.g. "ws://your-validator:8443") */
  url: string;
  /** WebSocket channel to connect to (default: "all") */
  channel?: Channel;
  /** Auto-reconnect on disconnect (default: true) */
  autoReconnect?: boolean;
  /** Reconnect delay in ms (default: 3000) */
  reconnectDelay?: number;
  /** Maximum reconnect attempts (default: Infinity) */
  maxReconnectAttempts?: number;
}

// ─── Client Events ──────────────────────────────────────────────────

export interface GatewayClientEvents {
  event: (event: ExecEvent) => void;
  events: (events: ExecEvent[]) => void;
  tps: (tps: number) => void;
  contention: (data: ContentionData) => void;
  topAccesses: (data: TopAccessesData) => void;
  connected: () => void;
  disconnected: () => void;
  error: (error: Error) => void;
}
