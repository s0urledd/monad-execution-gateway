# Wire Protocol Specification

> Defines the exact format of every byte sent over WebSocket connections.
> This document is the single source of truth for SDK implementors.

---

## 1. Transport

| Property | Value |
|----------|-------|
| Protocol | WebSocket (RFC 6455) |
| Frame type | Text (JSON) |
| Encoding | UTF-8 |
| Compression | None (per-message deflate not enabled) |
| Max frame size | Unlimited (but see Section 6 for batching rules) |

---

## 2. Message Envelope

Every server-to-client message is a JSON object with exactly two top-level keys:

```json
{
  "server_seqno": <u64>,
  "<MessageType>": <payload>
}
```

### 2.1 Fields

| Field | Type | Description |
|-------|------|-------------|
| `server_seqno` | `u64` | Monotonically increasing sequence number. Unique per message across all connections. |
| `<MessageType>` | varies | Exactly one of the message types defined in Section 3. The key name is the discriminator. |

### 2.2 Serialization Rules

- Keys are serialized using Rust's `serde_json` default: struct field order as declared.
- `server_seqno` always appears first (it is the first field in the `WireMessage` struct).
- Numeric types: integers as JSON numbers, no string-encoding.
- Hex-encoded types: `B256` and `Address` use `alloy_primitives` serde (0x-prefixed lowercase hex).
- `null` omission: Fields annotated with `#[serde(skip_serializing_if = "Option::is_none")]` are omitted when `None`.
- Byte arrays (`Bytes`): 0x-prefixed hex string.

---

## 3. Message Types

### 3.1 `Hello`

Sent as the **first frame** on every connection. Declares server capabilities.

```json
{
  "server_seqno": 0,
  "Hello": {
    "wire_version": 1,
    "server_version": "0.1.0",
    "capabilities": ["lifecycle", "contention", "resume", "heartbeat"]
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `wire_version` | `u32` | Protocol version. Incremented on breaking changes. |
| `server_version` | `string` | Gateway software version (semver). |
| `capabilities` | `string[]` | Feature flags the server supports. |

### 3.2 `Resume`

Sent as the **second frame** on every connection (after `Hello`). Indicates resume mode.

```json
{
  "server_seqno": 0,
  "Resume": {
    "mode": "resume"
  }
}
```

| Field | Type | Values |
|-------|------|--------|
| `mode` | `string` | `"resume"` (cursor valid, replay follows) or `"snapshot"` (fresh state) |

### 3.3 `Events`

A batch of one or more execution events.

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
      "payload": {
        "type": "BlockStart",
        "block_number": 56147820,
        "block_id": "0x...",
        ...
      },
      "seqno": 9876543210,
      "timestamp_ns": 1708345678000000000
    }
  ]
}
```

**Event envelope fields:**

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `event_name` | `string` | No | PascalCase event type name |
| `block_number` | `u64` | Yes | Block number this event belongs to |
| `txn_idx` | `usize` | Yes | Transaction index within the block |
| `txn_hash` | `string` | Yes | 0x-prefixed hex transaction hash |
| `commit_stage` | `string` | Yes | Block's current consensus stage |
| `payload` | `object` | No | Event-specific data, discriminated by `type` field |
| `seqno` | `u64` | No | Event ring sequence number (from validator) |
| `timestamp_ns` | `u64` | No | Nanosecond unix timestamp from event ring |

### 3.4 `TPS`

```json
{"server_seqno": 43, "TPS": 2450}
```

Payload is a bare integer (transactions per second estimate).

### 3.5 `ContentionData`

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

See [EVENTS.md](EVENTS.md) for full field definitions.

### 3.6 `TopAccesses`

```json
{
  "server_seqno": 45,
  "TopAccesses": {
    "account": [{"key": "0x...", "count": 15234}],
    "storage": [{"key": ["0x...", "0x..."], "count": 8921}]
  }
}
```

### 3.7 `Lifecycle`

Block stage transition event.

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

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `block_hash` | `string` | No | 0x-prefixed consensus block ID |
| `block_number` | `u64` | No | Block number |
| `from_stage` | `string` | Yes | Previous stage (null for initial Proposed) |
| `to_stage` | `string` | No | New stage |
| `time_in_previous_stage_ms` | `f64` | Yes | Milliseconds spent in previous stage |
| `block_age_ms` | `f64` | No | Total milliseconds since Proposed |
| `txn_count` | `usize` | No | Number of transactions in the block |
| `gas_used` | `u64` | Yes | Gas used (available after BlockEnd) |

---

## 4. Client-to-Server Messages

### 4.1 Subscribe (Simple)

```json
{"subscribe": ["BlockStart", "BlockFinalized", "TPS"]}
```

### 4.2 Subscribe (Advanced)

```json
{
  "subscribe": {
    "events": ["TxnLog"],
    "filters": [
      {
        "event_name": "TxnLog",
        "field_filters": [
          {"field": "address", "filter": {"values": ["0x..."]}}
        ]
      }
    ],
    "min_stage": "Finalized"
  }
}
```

### 4.3 Hello (Optional)

```json
{
  "hello": {
    "wire_version": 1,
    "client_name": "my-bot",
    "client_version": "1.0.0"
  }
}
```

### 4.4 Ping/Pong

Standard WebSocket Ping/Pong frames (binary, handled at protocol level).

---

## 5. Query Parameters

All WebSocket endpoints accept:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `resume_from` | `u64` | None | Last seen `server_seqno`. Server replays from ring buffer if available. |

---

## 6. Batching Rules

### 6.1 Event Batching

Multiple events from the same broadcast cycle are batched into a single `Events` message. This happens naturally when the server drains the broadcast channel in a tight loop.

| Parameter | Value | Description |
|-----------|-------|-------------|
| Max events per message | Unbounded (drain-based) | All available events in the current tick are batched |
| Target frame size | No hard limit | Practical limit is ~1MB (WebSocket library default) |

### 6.2 Metric Messages

Metric messages (`TPS`, `ContentionData`, `TopAccesses`, `Lifecycle`) are sent as separate frames, not batched with `Events`.

### 6.3 Ordering Within a Batch

Events within an `Events` array are in the order they were broadcast — which is the order they were received from the event ring.

---

## 7. Versioning

### 7.1 Wire Version

| Version | Description |
|---------|-------------|
| `1` | Initial version. Current protocol as defined in this document. |

### 7.2 Compatibility Rules

- **Non-breaking changes** (new optional fields, new message types): Do not increment `wire_version`. Clients must ignore unknown fields/types.
- **Breaking changes** (removed fields, changed semantics, renamed types): Increment `wire_version`.
- The server declares its `wire_version` in the `Hello` message.

### 7.3 SDK Requirements

SDKs must:
- Send a `Hello` response with their supported `wire_version`.
- Log a warning if the server's `wire_version` differs from the client's.
- Continue operating on version mismatch (best effort).

---

## 8. Event Type Catalog

All event types and their `payload.type` discriminator values:

| `event_name` | `payload.type` | Category |
|-------------|----------------|----------|
| `RecordError` | `RecordError` | Error |
| `BlockStart` | `BlockStart` | Block lifecycle |
| `BlockReject` | `BlockReject` | Block lifecycle |
| `BlockPerfEvmEnter` | `BlockPerfEvmEnter` | Block perf |
| `BlockPerfEvmExit` | `BlockPerfEvmExit` | Block perf |
| `BlockEnd` | `BlockEnd` | Block lifecycle |
| `BlockQC` | `BlockQC` | Block lifecycle |
| `BlockFinalized` | `BlockFinalized` | Block lifecycle |
| `BlockVerified` | `BlockVerified` | Block lifecycle |
| `TxnHeaderStart` | `TxnHeaderStart` | Txn lifecycle |
| `TxnAccessListEntry` | `TxnAccessListEntry` | Txn detail |
| `TxnAuthListEntry` | `TxnAuthListEntry` | Txn detail |
| `TxnHeaderEnd` | `TxnHeaderEnd` | Txn lifecycle |
| `TxnReject` | `TxnReject` | Txn lifecycle |
| `TxnPerfEvmEnter` | `TxnPerfEvmEnter` | Txn perf |
| `TxnPerfEvmExit` | `TxnPerfEvmExit` | Txn perf |
| `TxnEvmOutput` | `TxnEvmOutput` | Txn lifecycle |
| `TxnLog` | `TxnLog` | Txn detail |
| `TxnCallFrame` | `TxnCallFrame` | Txn detail |
| `TxnEnd` | `TxnEnd` | Txn lifecycle |
| `AccountAccessListHeader` | `AccountAccessListHeader` | State access |
| `AccountAccess` | `AccountAccess` | State access |
| `StorageAccess` | `StorageAccess` | State access |
| `EvmError` | `EvmError` | Error |

### 8.1 Subscribable Meta-Items

These are not event types but can be included in subscribe messages:

| Item | Message Type | Description |
|------|-------------|-------------|
| `TPS` | `TPS` | TPS metric updates |
| `ContentionData` | `ContentionData` | Per-block contention analytics |
| `TopAccesses` | `TopAccesses` | Top accessed accounts/storage |
| `Lifecycle` | `Lifecycle` | Block stage transitions |

---

## 9. Connection Lifecycle

### 9.1 Normal Flow

```
Client                                          Server
  │                                               │
  │  WebSocket upgrade (?resume_from=N)           │
  ├──────────────────────────────────────────────►│
  │                                               │
  │  Hello {wire_version, capabilities}           │
  │◄──────────────────────────────────────────────┤  frame 1
  │                                               │
  │  Resume {mode}                                │
  │◄──────────────────────────────────────────────┤  frame 2
  │                                               │
  │  [replay messages if mode="resume"]           │
  │◄──────────────────────────────────────────────┤  frames 3..N
  │                                               │
  │  [live messages]                              │
  │◄──────────────────────────────────────────────┤  ongoing
  │                                               │
  │  Subscribe {events, filters}                  │
  ├──────────────────────────────────────────────►│  (optional, up to 5x)
  │                                               │
  │  Ping                                         │
  │◄──────────────────────────────────────────────┤  every 30s
  │  Pong                                         │
  ├──────────────────────────────────────────────►│
  │                                               │
  │  Close                                        │
  ├──────────────────────────────────────────────►│
```

### 9.2 Error Cases

| Error | Server Behavior |
|-------|-----------------|
| Per-IP limit exceeded | HTTP 429 (no WebSocket upgrade) |
| Subscribe limit exceeded | Message silently ignored |
| Slow client (10K drops) | Server closes connection |
| Client Pong timeout | Server closes connection |
| Invalid JSON from client | Message ignored |
| Server shutdown (SIGTERM) | Server closes all connections |
