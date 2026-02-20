# Execution Events Reference

Monad's execution engine emits structured events from the EVM in real-time via a memory-mapped ring buffer. These events provide **execution-level visibility** that is not available through standard Ethereum JSON-RPC.

> **Wire format:** Events are delivered inside an `Events` batch on the WebSocket. Every wire message carries a `server_seqno` for [cursor resume](API.md#cursor-resume):
> ```json
> {"server_seqno": 42, "Events": [{ ... }, { ... }]}
> ```
> See the [API Reference](API.md) for the full wire format.

## Event Envelope

Each event inside the `Events` array has the following fields:

| Field | Type | Description |
|-------|------|-------------|
| `event_name` | string | Event type identifier |
| `block_number` | number \| null | Block height |
| `txn_idx` | number \| null | Transaction index within block |
| `txn_hash` | string \| null | Transaction hash (0x-prefixed) |
| `commit_stage` | string \| null | Block's current consensus stage (see below) |
| `seqno` | number | Sequence number from the event ring |
| `timestamp_ns` | number | Nanosecond-precision unix timestamp |
| `payload` | object | Event-specific data (discriminated by `type` field) |

### `commit_stage` — Finality Confidence

Every event carries its block's current `commit_stage`, indicating how far the block has progressed through MonadBFT consensus:

| Stage | Meaning | Use Case |
|-------|---------|----------|
| `Proposed` | Block proposed, execution started | Lowest latency, speculative |
| `Voted` | QC received (~400ms) | Speculative finality, safe for most reads |
| `Finalized` | Irreversibly committed (~800ms) | Full finality, safe for state changes |
| `Verified` | State root verified | Terminal confirmation |
| `Rejected` | Block dropped | Discard associated data |

Use `min_stage` in subscriptions to only receive events from blocks at or above a specific stage. For example, `"min_stage": "Finalized"` ensures you only process events from irreversibly committed blocks.

## Block Lifecycle Events

### `BlockStart`

Emitted when block execution begins. Triggers the **Proposed** lifecycle stage.

| Field | Type | Description |
|-------|------|-------------|
| `block_number` | number | Block height |
| `block_id` | string | Monad consensus block ID |
| `round` | number | Consensus round |
| `epoch` | number | Consensus epoch |
| `parent_eth_hash` | string | Parent block's Ethereum hash |
| `timestamp` | number | Block timestamp (unix seconds) |
| `beneficiary` | string | Block producer address |
| `gas_limit` | number | Block gas limit |
| `base_fee_per_gas` | string | EIP-1559 base fee (hex U256) |

### `BlockEnd`

Emitted when block execution completes. Updates internal execution metadata (gas_used, eth_block_hash) but does **not** change the public lifecycle stage.

| Field | Type | Description |
|-------|------|-------------|
| `eth_block_hash` | string | Ethereum block hash |
| `state_root` | string | Post-execution state root |
| `receipts_root` | string | Receipts trie root |
| `logs_bloom` | string | Bloom filter for logs |
| `gas_used` | number | Total gas consumed |

### `BlockQC`

Emitted when the block receives a Quorum Certificate (2/3+ validator votes). Triggers the **Voted** lifecycle stage (~400ms, speculative finality).

| Field | Type | Description |
|-------|------|-------------|
| `block_id` | string | Block ID |
| `block_number` | number | Block height |
| `round` | number | Consensus round |

### `BlockFinalized`

Emitted when the block is finalized (irreversible). Triggers the **Finalized** lifecycle stage (~800ms, full finality).

| Field | Type | Description |
|-------|------|-------------|
| `block_id` | string | Block ID |
| `block_number` | number | Block height |

### `BlockVerified`

Emitted when block execution results are verified. Triggers the **Verified** lifecycle stage (terminal).

| Field | Type | Description |
|-------|------|-------------|
| `block_number` | number | Block height |

### `BlockReject`

Emitted when a block proposal is rejected. Triggers the **Rejected** lifecycle stage (terminal).

| Field | Type | Description |
|-------|------|-------------|
| `reason` | number | Rejection reason code |

### `BlockPerfEvmEnter` / `BlockPerfEvmExit`

Performance markers for block-level EVM execution. Use the timestamp difference to measure total EVM wall time per block.

## Transaction Lifecycle Events

### `TxnHeaderStart`

Emitted when transaction execution begins. Contains the full transaction header.

| Field | Type | Description |
|-------|------|-------------|
| `txn_index` | number | Index within the block |
| `txn_hash` | string | Transaction hash |
| `sender` | string | Sender address |
| `txn_type` | number | EIP-2718 transaction type (0=legacy, 2=EIP-1559) |
| `nonce` | number | Sender nonce |
| `gas_limit` | number | Gas limit |
| `max_fee_per_gas` | string | Max fee per gas (hex U256) |
| `max_priority_fee_per_gas` | string | Max priority fee (hex U256) |
| `value` | string | Transfer value (hex U256) |
| `data` | string | Calldata (hex bytes) |
| `to` | string | Recipient address |
| `is_contract_creation` | boolean | Whether this creates a contract |

### `TxnEvmOutput`

Emitted when EVM execution completes for a transaction.

| Field | Type | Description |
|-------|------|-------------|
| `txn_index` | number | Transaction index |
| `log_count` | number | Number of logs emitted |
| `status` | boolean | true = success, false = revert |
| `gas_used` | number | Actual gas consumed |

### `TxnLog`

EVM log event (equivalent to Solidity `emit`).

| Field | Type | Description |
|-------|------|-------------|
| `txn_index` | number | Transaction index |
| `log_index` | number | Log index within the transaction |
| `address` | string | Emitting contract address |
| `topics` | string | Concatenated 32-byte topics (hex) |
| `data` | string | Log data (hex bytes) |

### `TxnCallFrame`

Internal call within a transaction (CALL, DELEGATECALL, STATICCALL, CREATE).

| Field | Type | Description |
|-------|------|-------------|
| `txn_index` | number | Transaction index |
| `depth` | number | Call stack depth |
| `caller` | string | Caller address |
| `call_target` | string | Target address |
| `value` | string | Value transferred (hex U256) |
| `input` | string | Call input data (hex) |
| `output` | string | Call return data (hex) |

### `TxnEnd`

Marks the end of transaction processing. No payload fields.

### `TxnReject`

Transaction rejected before execution.

| Field | Type | Description |
|-------|------|-------------|
| `txn_index` | number | Transaction index |
| `reason` | number | Rejection reason code |

### `TxnPerfEvmEnter` / `TxnPerfEvmExit`

Performance markers for per-transaction EVM execution timing.

## State Access Events

These events provide **execution-level storage visibility** — the core differentiator from standard RPC.

### `AccountAccess`

Emitted when the EVM accesses an account's state.

| Field | Type | Description |
|-------|------|-------------|
| `txn_index` | number \| null | Transaction that triggered this access |
| `address` | string | Account address |
| `balance` | string | Account balance (hex U256) |
| `nonce` | number | Account nonce |
| `code_hash` | string | Contract code hash |

### `StorageAccess`

Emitted when the EVM reads or writes a storage slot.

| Field | Type | Description |
|-------|------|-------------|
| `txn_index` | number \| null | Transaction that triggered this access |
| `account_index` | number | Index referencing the account |
| `key` | string | Storage slot key (bytes32) |
| `value` | string | Storage slot value (bytes32) |

### `AccountAccessListHeader`

Header for a batch of account accesses.

| Field | Type | Description |
|-------|------|-------------|
| `txn_index` | number \| null | Transaction index |
| `entry_count` | number | Number of account access entries following |

## Error Events

### `RecordError`

Emitted when the event ring encounters a recording error (e.g. payload too large).

| Field | Type | Description |
|-------|------|-------------|
| `error_type` | number | Error type code |
| `dropped_event_type` | number | Event type that was dropped |
| `truncated_payload_size` | number | Truncated size |
| `requested_payload_size` | number | Originally requested size |

### `EvmError`

EVM-level execution error.

| Field | Type | Description |
|-------|------|-------------|
| `domain_id` | number | Error domain |
| `status_code` | number | Error status code |
