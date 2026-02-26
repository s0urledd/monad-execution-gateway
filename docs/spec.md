# Behavioral Specification

> Normative contract for the Monad Execution Events Gateway.
> Any deviation from this document is a bug.

---

## 1. Event Ordering Guarantees

### 1.1 Global Ordering

Every message sent on any WebSocket channel carries a `server_seqno` field.

| Property | Guarantee |
|----------|-----------|
| Monotonic | `server_seqno` is strictly increasing across all messages on a single connection. |
| Gap-free (live) | In live mode, consecutive messages on a connection have consecutive seqnos **unless** backpressure drops occurred (see Section 4). |
| Unique | No two broadcast items share the same `server_seqno`. |

`server_seqno` is a 64-bit unsigned integer starting from 1 at gateway startup. It does **not** persist across restarts (see Section 5).

### 1.2 Per-Block Ordering

Within a single block, events arrive in execution order:

```
BlockStart
  TxnHeaderStart(0) ... TxnEnd(0)
  TxnHeaderStart(1) ... TxnEnd(1)
  ...
  TxnHeaderStart(N) ... TxnEnd(N)
BlockEnd
```

Events from different blocks may interleave if the validator streams overlapping execution. The gateway does **not** reorder cross-block events.

### 1.3 Per-Transaction Ordering

Within a single transaction, events follow this order:

```
TxnHeaderStart
  TxnAccessListEntry*
  TxnAuthListEntry*
TxnHeaderEnd
  TxnPerfEvmEnter
    AccountAccess* / StorageAccess*
    TxnLog*
    TxnCallFrame*
  TxnPerfEvmExit
TxnEvmOutput
TxnEnd
```

Optional events (marked `*`) may appear zero or more times.

### 1.4 Lifecycle Event Ordering

Block lifecycle updates are emitted **after** the corresponding raw event but **within the same server_seqno batch**. The lifecycle update for a stage transition is always broadcast after the raw event that triggered it.

Lifecycle stage progression is monotonic per-block:

```
Proposed → Voted → Finalized → Verified
                                    │
          (any stage) ──────────► Rejected
```

Skipping stages is allowed (e.g., Proposed → Finalized if the gateway starts mid-stream). Backward transitions are rejected and never emitted.

---

## 2. Block Lifecycle State Machine

### 2.1 States

| State | Entry Event | Terminal |
|-------|-------------|----------|
| `Proposed` | `BlockStart` | No |
| `Voted` | `BlockQC` | No |
| `Finalized` | `BlockFinalized` | No |
| `Verified` | `BlockVerified` | Yes |
| `Rejected` | `BlockReject` | Yes |

### 2.2 Transition Rules

1. **Monotonic progression**: The ordinal value of the next state must be strictly greater than the current state. Exception: `Rejected` is always valid from any non-terminal state.
2. **Skip allowed**: A block may jump from `Proposed` directly to `Finalized` if intermediate events were missed.
3. **Terminal states are final**: Once a block reaches `Verified` or `Rejected`, no further transitions are accepted.
4. **Unknown blocks are ignored**: If a lifecycle event arrives for a block not in the tracker, it is silently dropped (no error emitted).

### 2.3 Internal vs Public Events

| Event | Public Stage Change | Metadata Update |
|-------|-------------------|--------------------|
| `BlockStart` | Proposed | Creates lifecycle entry |
| `BlockEnd` | None | Sets `gas_used`, `eth_block_hash`, `execution_end_ns` |
| `BlockQC` | Voted | Records timestamp |
| `BlockFinalized` | Finalized | Records timestamp, emits `finalize_latency_ms` metric |
| `BlockVerified` | Verified | Records timestamp, moves to completed |
| `BlockReject` | Rejected | Records timestamp, moves to completed |
| `BlockPerfEvmEnter/Exit` | None | None (informational) |

### 2.4 Retention

- **Active blocks**: Kept in a `HashMap` keyed by `block_hash`. No limit on active count (bounded by block production rate).
- **Completed blocks**: Kept in a bounded `VecDeque`. Default capacity: 128. Oldest evicted first (FIFO).
- **Number-to-hash index**: Entries removed when the corresponding completed block is evicted.

### 2.5 Commit Stage Tagging

Every event carries a `commit_stage` field reflecting the block's **current** public stage at the time the event is broadcast. This is a snapshot — the same event may have had a different `commit_stage` if queried later.

For stage-aware filtering (`min_stage`), the server checks `commit_stage >= min_stage`. Events with `commit_stage = null` (block not tracked) pass through.

---

## 3. Subscription Semantics

### 3.1 Default Subscriptions

Each channel has a default subscription applied on connect:

| Channel | Default Events | Default Metrics |
|---------|---------------|-----------------|
| `/v1/ws` | All events | TPS, Contention, TopAccesses, Lifecycle |
| `/v1/ws/blocks` | Block lifecycle events | TPS, Lifecycle |
| `/v1/ws/txs` | Transaction events | None |
| `/v1/ws/contention` | None | Contention |
| `/v1/ws/lifecycle` | None | Lifecycle |

### 3.2 Subscribe Protocol

Clients may send a JSON text frame to update their subscription. Each subscribe message **replaces** the current subscription entirely. Identical subscriptions (same as current) are silently ignored.

### 3.3 Filter Evaluation Order

For each broadcast item:

1. **Base filter** (restricted mode): If the gateway runs in restricted mode, the base filter is applied first. Events not matching are dropped for all clients.
2. **Event name filter**: If the client's subscription specifies event names, only matching events pass.
3. **Field filter**: If field-level filters are specified, all must match (AND logic).
4. **Stage filter** (`min_stage`): If set, the event's `commit_stage` must be >= the requested stage.

### 3.4 Stage-Aware Filtering Behavior

When `min_stage` is set:
- Events from blocks at or beyond the requested stage are delivered **immediately**.
- Events from blocks below the requested stage are **dropped** (not buffered).
- Events with unknown `commit_stage` (null) are **delivered** (fail-open).

> **Note**: The current implementation drops events below `min_stage` rather than buffering them. This is a deliberate trade-off for simplicity. A future version may buffer and flush.

---

## 4. Backpressure Policy

### 4.1 Architecture

```
Broadcast channel ─► Per-client bounded mpsc (4096) ─► WebSocket send task
```

### 4.2 Drop Behavior

| Condition | Action |
|-----------|--------|
| Channel has capacity | Message enqueued, `total_sent` incremented |
| Channel full | Message **dropped**, `drop_count` incremented, `ws_dropped_total` metric incremented |
| `drop_count` reaches 1,000 (and every 1,000 after) | Warning logged server-side; `Warning` frame queued for client (best-effort) |
| `drop_count` reaches 10,000 | Client **disconnected** |
| Channel receiver closed | Client **disconnected** |

### 4.3 Client-Facing Warning Frame

Every 1,000 cumulative drops, the server injects a `Warning` frame into the client's send channel on the next successful send:

```json
{"server_seqno": 0, "Warning": {"type": "backpressure", "dropped": 1000, "drop_limit": 10000}}
```

This is **best-effort** — if the channel is still full when the warning is attempted, it is silently skipped. The `backpressure_notify` feature flag in the `Hello` message advertises this capability.

### 4.4 Invariants

- The broadcast channel itself (1,000,000 capacity) is never the bottleneck. If it fills, the system is critically overloaded.
- Backpressure is **per-client**. A slow client does not affect other clients.
- Dropped messages are lost. The client must use `resume_from` on reconnect to recover them (if still in the ring buffer).

---

## 5. Resume Semantics

### 5.1 Resume Protocol

On every WebSocket connect, the server sends a `Resume` control message as the **second frame** (after `Hello`):

```json
{"server_seqno": 0, "Resume": {"mode": "<resume|snapshot>"}}
```

### 5.2 Resume Decision Logic

| Condition | Mode | Behavior |
|-----------|------|----------|
| `resume_from` provided AND seqno is within ring buffer range | `resume` | Replay all buffered messages with `seqno > resume_from`, then switch to live |
| `resume_from` provided AND seqno is older than ring buffer | `snapshot` | Send current state snapshot (TPS, contention, lifecycle), then live |
| `resume_from` provided AND ring buffer is empty | `snapshot` | Fallback to snapshot |
| No `resume_from` | `snapshot` | Fresh connect, send snapshot |

### 5.3 Ring Buffer Properties

| Property | Value |
|----------|-------|
| Capacity | 100,000 entries |
| Entry type | Pre-serialized JSON strings |
| Eviction | FIFO (oldest removed when full) |
| Memory bound | ~20-100 MB depending on event size |
| Persistence | **None** — in-memory only, lost on restart |

### 5.4 Replay Guarantees

- Replayed messages are byte-identical to the original broadcast.
- `server_seqno` values in replayed messages are the same as originally assigned.
- After replay completes, the server transitions to live mode. There is a brief overlap window where a live message may duplicate the last replayed message — clients should deduplicate by `server_seqno`.

### 5.5 Server Restart Behavior

On gateway restart:
- `server_seqno` resets to 0.
- Ring buffer is empty.
- All clients reconnecting with `resume_from` will receive `"snapshot"` mode.
- Clients must detect the seqno discontinuity (new seqno < last seen) and treat it as a full reset.

---

## 6. Access Model

The gateway binds to `127.0.0.1` (localhost) by default, meaning it is **not reachable from the network** unless the operator explicitly changes the bind address. This is intentional — the gateway is designed to run co-located with the Monad validator and serve local consumers (SDKs, webhook relay, monitoring).

If remote access is needed, place the gateway behind a reverse proxy (nginx, Caddy, etc.) with TLS and authentication. The gateway itself does not enforce rate limits, authentication, or IP allowlists — these are the responsibility of the deployment layer.

---

## 7. Graceful Shutdown

### 7.1 Trigger

SIGINT or SIGTERM signal.

### 7.2 Sequence

1. Stop accepting new connections (Axum graceful shutdown).
2. Send shutdown signal via `watch` channel to all WebSocket handlers.
3. Each handler breaks out of the event loop and closes the WebSocket.
4. Sender tasks drain remaining messages and close.
5. Process exits after all connections close.

### 7.3 Timeout

No explicit shutdown timeout. If clients don't close, the process will eventually be killed by the orchestrator (Docker/systemd).

---

## 8. Health Check

### 8.1 Endpoint

`GET /health`

### 8.2 Behavior

| Condition | Response | Side Effect |
|-----------|----------|-------------|
| Events received within 10s (or no events yet) | `{"success": true}` | None |
| No events for 10-29s (at least one event received) | `{"success": false}` | None |
| No events for 30+s AND at least one event received | Process exits (exit code 1) | Triggers container restart |

### 8.3 Status Endpoint

`GET /v1/status` provides a non-fatal health indicator:

| `status` value | Condition |
|----------------|-----------|
| `"healthy"` | Last event within 10s, or no events received yet |
| `"degraded"` | Last event older than 10s |

---

## 9. Heartbeat Protocol

### 9.1 Server-Side Ping

The server sends WebSocket Ping frames at a configurable interval (default: 30 seconds).

### 9.2 Client Liveness

If no data (Pong or any message) is received from a client within the liveness timeout (default: 2x ping interval = 60 seconds), the server closes the connection.

### 9.3 Client-Side Ping

Clients may send Ping frames at any time. The server responds with Pong (handled by the WebSocket library).

---

## 10. Wire Version Handshake

### 10.1 Protocol

On WebSocket connect, the server sends a `Hello` control message as the **first frame** (before `Resume`). This is a one-way declaration — no negotiation.

```json
{
  "server_seqno": 0,
  "Hello": {
    "wire_version": 1,
    "server_version": "0.1.0",
    "features": ["lifecycle", "contention", "resume", "heartbeat", "stage_filter", "backpressure_notify"],
    "limits": {
      "resume_buffer_size": 100000,
      "client_send_buffer": 4096,
      "slow_client_drop_limit": 10000,
      "heartbeat_interval_secs": 30,
      "heartbeat_timeout_secs": 60
    }
  }
}
```

The `features` array lists server capabilities. The `limits` object exposes operational parameters so clients can tune their behavior (e.g., reconnect strategy based on `resume_buffer_size`).

### 10.2 Client Response (Optional)

Clients may send a `Hello` message to identify themselves. This is purely informational — the server logs it but takes no action. If the client never sends a Hello, nothing changes.

```json
{"hello": {"wire_version": 1, "client_name": "my-bot", "client_version": "1.0.0"}}
```

### 10.3 Version Mismatch

- The server declares the wire protocol version it speaks.
- If the client sends a `hello` with a different `wire_version`, the server logs the mismatch but continues (forward-compatible).
- Breaking wire format changes increment `wire_version`. Non-breaking additions do not.
