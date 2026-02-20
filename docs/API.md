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
| `GET /v1/status` | Gateway status, cursor resume window |
| `GET /health` | Health check for monitoring/orchestration |

---

## Wire Format

Every message from the server is a JSON text frame with a `server_seqno` field and exactly one data key:

```json
{
  "server_seqno": 12345,
  "Events": [...]
}
```

The `server_seqno` is a monotonic counter that uniquely identifies each wire message. Clients should track the last seen value and pass it back as `?resume_from=<seqno>` on reconnect.

---

## Cursor Resume

All WebSocket endpoints accept an optional `resume_from` query parameter:

```
ws://host:8443/v1/ws?resume_from=12345
```

### How It Works

1. The server keeps a ring buffer of the last 100,000 pre-serialized wire messages.
2. On connect with `?resume_from=N`, the server replays all buffered messages with `server_seqno > N`.
3. The first frame is always a `Resume` control message indicating the mode.

### Resume Control Message

Sent as the first frame after every connect/reconnect:

```json
{"server_seqno": 42, "Resume": {"mode": "resume"}}
```

| `mode` | Meaning |
|--------|---------|
| `"resume"` | Cursor was valid. Buffered messages follow — no data loss. |
| `"snapshot"` | Cursor was too old or fresh connect. Snapshot state follows (TPS, contention, lifecycle). |

### Checking the Resume Window

Before connecting, clients can check if their cursor is still valid:

```bash
curl http://host:8443/v1/status
```

```json
{
  "server_seqno": 54321,
  "oldest_seqno": 4321,
  "newest_seqno": 54321,
  ...
}
```

If `your_cursor >= oldest_seqno`, resume will succeed. Otherwise you'll get a snapshot fallback.

---

## Backpressure

Each WebSocket client gets a bounded send buffer of **4096 messages**. When the buffer is full (client consuming too slowly):

- New messages are **dropped** (not queued)
- Warning logged every 1,000 drops
- Client **disconnected** after 10,000 cumulative drops

On disconnect, the server logs `sent` and `dropped` counts. This prevents a single slow consumer from exhausting server memory.

---

## WebSocket

### Connection

```
ws://<GATEWAY_HOST>:8443/v1/ws
ws://<GATEWAY_HOST>:8443/v1/ws?resume_from=12345
```

No authentication required. Once connected, the server sends a `Resume` control frame followed by live data.

### Channel Details

**`/v1/ws`** — Full firehose. All execution events, TPS, contention, and top-K accesses.

**`/v1/ws/blocks`** — Block lifecycle only: `BlockStart`, `BlockEnd`, `BlockQC`, `BlockFinalized`, `BlockVerified`, `BlockReject`, plus TPS metrics.

**`/v1/ws/txs`** — Transaction events: `TxnHeaderStart`, `TxnEvmOutput`, `TxnLog`, `TxnCallFrame`, `TxnEnd`, `TxnReject`, `NativeTransfer`.

**`/v1/ws/contention`** — Only `ContentionData` messages (per-block contention analytics).

**`/v1/ws/lifecycle`** — Block stage transitions only. Each message is a `Lifecycle` update showing a block advancing through MonadBFT consensus.

### Client-Driven Subscriptions

After connecting, clients can send a subscribe message to filter events server-side.

**Simple format:**

```json
{
  "subscribe": ["BlockFinalized", "BlockStart", "TPS"]
}
```

**Advanced format** — with field-level filters and stage-aware filtering:

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
    ],
    "min_stage": "Finalized"
  }
}
```

Valid `min_stage` values: `"Proposed"`, `"Voted"`, `"Finalized"`, `"Verified"`. Events from blocks that haven't reached the requested stage are silently dropped.

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

Every message is a JSON object with `server_seqno` and **exactly one** data key.

#### `Resume`

Control message, always the first frame after connect/reconnect.

```json
{"server_seqno": 0, "Resume": {"mode": "snapshot"}}
```

#### `Events`

A batch of execution events.

```json
{
  "server_seqno": 42,
  "Events": [
    {
      "event_name": "BlockStart",
      "block_number": 56147820,
      "txn_idx": null,
      "txn_hash": null,
      "commit_stage": "Proposed",
      "payload": { "type": "BlockStart", ... },
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
| `commit_stage` | string \| null | Block's current consensus stage |
| `payload` | object | Event-specific data (discriminated by `type` field) |
| `seqno` | number | Sequence number from the event ring |
| `timestamp_ns` | number | Nanosecond-precision unix timestamp |

#### `TPS`

```json
{"server_seqno": 43, "TPS": 2450}
```

#### `ContentionData`

Per-block storage contention analytics. Sent once per block on `BlockEnd`.

```json
{
  "server_seqno": 44,
  "ContentionData": {
    "block_number": 56147820,
    "block_wall_time_ns": 45000000,
    "total_tx_time_ns": 350000000,
    "parallel_efficiency_pct": 87.14,
    "total_unique_slots": 1523,
    "contended_slot_count": 42,
    "contention_ratio": 0.0275,
    "total_txn_count": 150,
    "top_contended_slots": [...],
    "top_contended_contracts": [...],
    "contract_edges": [...]
  }
}
```

#### `TopAccesses`

Top accessed accounts and storage slots (Space-Saving algorithm, reset every 5 minutes).

```json
{
  "server_seqno": 45,
  "TopAccesses": {
    "account": [{ "key": "0x...", "count": 15234 }],
    "storage": [{ "key": ["0x...", "0x..."], "count": 8921 }]
  }
}
```

#### `Lifecycle`

Block stage transition through MonadBFT consensus.

```json
{
  "server_seqno": 46,
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

**Block Stages** (MonadBFT consensus):

| Stage | Meaning | Typical Timing |
|-------|---------|----------------|
| `Proposed` | Block proposed for execution | t=0 |
| `Voted` | Quorum certificate received (2/3+ validators) | ~400ms |
| `Finalized` | Committed to canonical chain (irreversible) | ~800ms |
| `Verified` | State root verified (terminal) | After finalization |
| `Rejected` | Dropped at any point (terminal) | Varies |

---

## REST

### `GET /v1/tps`

```json
{ "tps": 2450 }
```

### `GET /v1/contention`

Latest per-block contention data (same schema as `ContentionData` WebSocket message payload).

### `GET /v1/status`

```json
{
  "status": "healthy",
  "block_number": 56147820,
  "connected_clients": 12,
  "uptime_secs": 86400,
  "last_event_age_secs": 0,
  "server_seqno": 54321,
  "oldest_seqno": 4321,
  "newest_seqno": 54321
}
```

| Field | Description |
|-------|-------------|
| `status` | `"healthy"` if events received within 10s, `"degraded"` otherwise |
| `block_number` | Latest block number seen |
| `connected_clients` | Number of active WebSocket connections |
| `server_seqno` | Current (newest) server sequence number |
| `oldest_seqno` | Oldest seqno still in the ring buffer (0 = buffer empty) |
| `newest_seqno` | Newest seqno assigned (same as `server_seqno`) |

### `GET /v1/blocks/lifecycle`

Lifecycle summaries for all recently tracked blocks. Returns an array of `BlockLifecycleSummary` objects.

### `GET /v1/blocks/:number/lifecycle`

Full lifecycle for a specific block by block number. Returns 404 if the block is not in the tracker's window.

### `GET /health`

Health check for Docker/orchestration. Returns `{"success": true}` when healthy. Process exits if no events for 30 seconds (triggers restart).

---

## Event Filtering

In **restricted mode** (default), the gateway applies server-side filters defined in `restricted_filters.json`. Only events matching the filter specs are sent to clients.

Set `ALLOW_UNRESTRICTED_FILTERS=1` to stream all events (useful for development or dedicated consumers).
