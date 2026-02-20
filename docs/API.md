# API Reference

## Endpoints

All endpoints are served on a single port (default `8443`).

### WebSocket Channels

| Endpoint | Description |
|----------|-------------|
| `/v1/ws` | All events + all computed metrics |
| `/v1/ws/blocks` | Block lifecycle events + TPS |
| `/v1/ws/txs` | Transaction events only |
| `/v1/ws/contention` | Contention data only |
| `/v1/ws/lifecycle` | Block stage transitions only (Proposed/Voted/Finalized/Verified) |
| `/` | Alias for `/v1/ws` (backward compatible) |

### REST Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /v1/tps` | Current TPS snapshot |
| `GET /v1/contention` | Latest per-block contention data |
| `GET /v1/blocks/lifecycle` | Lifecycle summaries for all tracked blocks |
| `GET /v1/blocks/:number/lifecycle` | Full lifecycle timeline for a specific block |
| `GET /v1/status` | Gateway status (block number, clients, uptime) |
| `GET /health` | Health check for monitoring/orchestration |

---

## WebSocket

### Connection

```
ws://<GATEWAY_HOST>:8443/v1/ws
```

No authentication required — connection is open. Once connected, the gateway immediately begins streaming events as JSON text frames.

### Channel Details

**`/v1/ws`** — Full firehose. All execution events, TPS, contention, and top-K accesses.

**`/v1/ws/blocks`** — Block lifecycle only: `BlockStart`, `BlockEnd`, `BlockQC`, `BlockFinalized`, `BlockVerified`, `BlockReject`, plus TPS metrics.

**`/v1/ws/txs`** — Transaction events: `TxnHeaderStart`, `TxnEvmOutput`, `TxnLog`, `TxnCallFrame`, `TxnEnd`, `TxnReject`, `NativeTransfer`.

**`/v1/ws/contention`** — Only `ContentionData` messages (per-block contention analytics).

**`/v1/ws/lifecycle`** — Block stage transitions only. Each message is a `Lifecycle` update showing a block advancing through MonadBFT consensus: Proposed → Voted → Finalized → Verified. Compact, high-signal stream for finality monitoring.

### Client-Driven Subscriptions

After connecting, clients can send a subscribe message to filter events server-side. This reduces bandwidth and processing on the client.

**Simple format** — list event names and metric types:

```json
{
  "subscribe": ["BlockFinalized", "BlockStart", "TPS"]
}
```

**Advanced format** — with field-level filters and/or stage-aware filtering:

```json
{
  "subscribe": {
    "events": ["TxnLog"],
    "filters": [
      {
        "event_name": "TxnLog",
        "field_filters": [
          {
            "field": "address",
            "filter": { "values": ["0x..."] }
          }
        ]
      }
    ]
  }
}
```

**Stage-aware filtering** — only receive events from blocks that have reached a minimum consensus stage:

```json
{
  "subscribe": {
    "events": ["TxnLog", "TxnEvmOutput"],
    "min_stage": "Finalized"
  }
}
```

Valid `min_stage` values: `"Proposed"`, `"Voted"`, `"Finalized"`, `"Verified"`. Events from blocks that haven't reached the requested stage are silently dropped. Default: no stage filter (events streamed immediately).

**Supported field filters:**

| Field | Filter Type | Applies To | Example |
|-------|-------------|------------|---------|
| `txn_index` | Range (`min`, `max`) | TxnLog | `{ "field": "txn_index", "filter": { "min": 0, "max": 5 } }` |
| `log_index` | Range (`min`, `max`) | TxnLog | `{ "field": "log_index", "filter": { "min": 0 } }` |
| `address` | Exact match (`values`) | TxnLog | `{ "field": "address", "filter": { "values": ["0x..."] } }` |
| `topics` | Array prefix (`values`) | TxnLog | `{ "field": "topics", "filter": { "values": ["0x..."] } }` |

Multiple field filters within a spec are combined with **AND** logic. Multiple filter specs in the `filters` array are combined with **OR** logic.

**Subscribable items:**
- Any event name (see [Events Reference](EVENTS.md)): `BlockStart`, `BlockFinalized`, `TxnLog`, etc.
- `TPS` — TPS metric updates
- `ContentionData` — Per-block contention analytics
- `TopAccesses` — Top accessed accounts and storage slots
- `Lifecycle` — Block stage transition updates

Subscriptions replace the current filter. Send a new subscribe message to change what you receive.

### Server Messages

Every message is a JSON object with **exactly one** key indicating the message type:

#### `Events`

A batch of execution events from the Monad event ring.

```json
{
  "Events": [
    {
      "event_name": "BlockStart",
      "block_number": 56147820,
      "txn_idx": null,
      "txn_hash": null,
      "payload": {
        "type": "BlockStart",
        "block_number": 56147820,
        "block_id": "0x...",
        "round": 12345,
        "epoch": 100,
        "parent_eth_hash": "0x...",
        "timestamp": 1708345678,
        "beneficiary": "0x...",
        "gas_limit": 30000000,
        "base_fee_per_gas": "0x..."
      },
      "seqno": 9876543210,
      "timestamp_ns": 1708345678000000000
    }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `event_name` | string | Event type (see [Events Reference](EVENTS.md)) |
| `block_number` | number \| null | Block number (if applicable) |
| `txn_idx` | number \| null | Transaction index within the block |
| `txn_hash` | string \| null | Transaction hash (0x-prefixed) |
| `commit_stage` | string \| null | Block's current consensus stage: `"Proposed"`, `"Voted"`, `"Finalized"`, `"Verified"`, or `"Rejected"` |
| `payload` | object | Event-specific data (discriminated by `type` field) |
| `seqno` | number | Monotonically increasing sequence number from the event ring |
| `timestamp_ns` | number | Nanosecond-precision unix timestamp |

The `commit_stage` field tells you the finality confidence of this event's block at the time it was processed. For example, a `TxnLog` event with `"commit_stage": "Finalized"` means the block is irreversibly committed.

#### `TPS`

Transactions per second computed over a 2.5-block rolling window (~1 second at Monad's ~400ms block time).

```json
{
  "TPS": 2450
}
```

#### `ContentionData`

Per-block storage contention analytics. Sent once per block on `BlockEnd`.

```json
{
  "ContentionData": {
    "block_number": 56147820,
    "block_wall_time_ns": 45000000,
    "total_tx_time_ns": 350000000,
    "parallel_efficiency_pct": 87.14,
    "total_unique_slots": 1523,
    "contended_slot_count": 42,
    "contention_ratio": 0.0275,
    "total_txn_count": 150,
    "top_contended_slots": [
      {
        "address": "0x...",
        "slot": "0x...",
        "txn_count": 8,
        "access_count": 15
      }
    ],
    "top_contended_contracts": [
      {
        "address": "0x...",
        "total_slots": 50,
        "contended_slots": 12,
        "total_accesses": 200,
        "contention_score": 0.24
      }
    ],
    "contract_edges": [
      {
        "contract_a": "0x...",
        "contract_b": "0x...",
        "shared_txn_count": 15
      }
    ]
  }
}
```

| Field | Description |
|-------|-------------|
| `parallel_efficiency_pct` | Percentage of execution time saved by parallel execution (higher = better) |
| `contention_ratio` | Fraction of storage slots accessed by 2+ transactions |
| `top_contended_slots` | Up to 20 most contended storage slots (by txn_count) |
| `top_contended_contracts` | Up to 15 contracts ranked by contention |
| `contract_edges` | Up to 15 contract pairs frequently co-accessed in same transactions |

#### `TopAccesses`

Top accessed accounts and storage slots (probabilistic top-K using Space-Saving algorithm, reset every 5 minutes).

```json
{
  "TopAccesses": {
    "account": [
      { "key": "0x...", "count": 15234 }
    ],
    "storage": [
      { "key": ["0x...", "0x..."], "count": 8921 }
    ]
  }
}
```

#### `Lifecycle`

Block stage transition. Emitted when a block advances through MonadBFT consensus: Proposed → Voted → Finalized → Verified.

```json
{
  "Lifecycle": {
    "block_hash": "0x...",
    "block_number": 56147820,
    "from_stage": "Proposed",
    "to_stage": "Voted",
    "time_in_previous_stage_ms": 412.5,
    "block_age_ms": 412.5,
    "txn_count": 150,
    "gas_used": null
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `block_hash` | string | Block hash (primary identifier) |
| `block_number` | number | Block height |
| `from_stage` | string \| null | Previous stage (null for `Proposed`) |
| `to_stage` | string | New stage reached |
| `time_in_previous_stage_ms` | number \| null | Time spent in previous stage (ms) |
| `block_age_ms` | number | Total time since `Proposed` (ms) |
| `txn_count` | number | Transactions in the block |
| `gas_used` | number \| null | Gas used (available after execution) |

**Block Stages** (MonadBFT consensus):

| Stage | Meaning | Typical Timing |
|-------|---------|----------------|
| `Proposed` | Block proposed for execution | t=0 |
| `Voted` | Quorum certificate received (2/3+ validators) | ~400ms (speculative finality) |
| `Finalized` | Committed to canonical chain (irreversible) | ~800ms (full finality) |
| `Verified` | State root verified (terminal) | After finalization |
| `Rejected` | Dropped at any point (terminal) | Varies |

---

## REST

### `GET /v1/tps`

Current TPS snapshot.

```json
{ "tps": 2450 }
```

### `GET /v1/contention`

Latest per-block contention data (same schema as the `ContentionData` WebSocket message payload).

### `GET /v1/status`

Gateway status.

```json
{
  "status": "healthy",
  "block_number": 56147820,
  "connected_clients": 12,
  "uptime_secs": 86400,
  "last_event_age_secs": 0
}
```

| Field | Description |
|-------|-------------|
| `status` | `"healthy"` if events received within 10s, `"degraded"` otherwise |
| `block_number` | Latest block number seen |
| `connected_clients` | Number of active WebSocket connections |

### `GET /v1/blocks/lifecycle`

Lifecycle summaries for all recently tracked blocks.

```json
[
  {
    "block_hash": "0x...",
    "block_number": 56147820,
    "current_stage": "Verified",
    "txn_count": 150,
    "gas_used": 12500000,
    "eth_block_hash": "0x...",
    "stage_timings_ms": {
      "Proposed": 0,
      "Voted": 412.5,
      "Finalized": 825.0,
      "Verified": 900.3
    },
    "execution_time_ms": 45.2,
    "total_age_ms": 900.3
  }
]
```

### `GET /v1/blocks/:number/lifecycle`

Full lifecycle for a specific block by block number. Returns a single `BlockLifecycleSummary` object (same schema as above). Returns 404 if the block is not in the tracker's window.

### `GET /health`

Health check for Docker/orchestration. Returns `{"success": true}` when healthy. Process exits if no events for 30 seconds (triggers restart).

---

## Event Filtering

In **restricted mode** (default), the gateway applies server-side filters defined in `restricted_filters.json`. Only events matching the filter specs are sent to clients.

Set `ALLOW_UNRESTRICTED_FILTERS=1` to stream all events (useful for development or dedicated consumers).
