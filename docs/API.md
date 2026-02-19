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
| `/` | Alias for `/v1/ws` (backward compatible) |

### REST Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /v1/tps` | Current TPS snapshot |
| `GET /v1/contention` | Latest per-block contention data |
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

### Client-Driven Subscriptions

After connecting, clients can send a subscribe message to filter events server-side. This reduces bandwidth and processing on the client.

**Simple format** — list event names and metric types:

```json
{
  "subscribe": ["BlockFinalized", "BlockStart", "TPS"]
}
```

**Advanced format** — with field-level filters:

```json
{
  "subscribe": {
    "events": ["AccountAccess"],
    "filters": [
      {
        "event_name": "AccountAccess",
        "field_name": "address",
        "field_value": "0x..."
      }
    ]
  }
}
```

**Subscribable items:**
- Any event name (see [Events Reference](EVENTS.md)): `BlockStart`, `BlockFinalized`, `TxnLog`, etc.
- `TPS` — TPS metric updates
- `ContentionData` — Per-block contention analytics
- `TopAccesses` — Top accessed accounts and storage slots

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
| `payload` | object | Event-specific data (discriminated by `type` field) |
| `seqno` | number | Monotonically increasing sequence number from the event ring |
| `timestamp_ns` | number | Nanosecond-precision unix timestamp |

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

### `GET /health`

Health check for Docker/orchestration. Returns `{"success": true}` when healthy. Process exits if no events for 30 seconds (triggers restart).

---

## Event Filtering

In **restricted mode** (default), the gateway applies server-side filters defined in `restricted_filters.json`. Only events matching the filter specs are sent to clients.

Set `ALLOW_UNRESTRICTED_FILTERS=1` to stream all events (useful for development or dedicated consumers).
