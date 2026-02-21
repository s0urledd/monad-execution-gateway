# Architecture

## System Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Monad Validator Node                         │
│                                                                      │
│  ┌────────────────┐     mmap (hugepages)     ┌────────────────────┐  │
│  │  Execution      │ ──────────────────────── │  Event Ring        │  │
│  │  Engine (EVM)   │   zero-copy writes       │  (shared memory)   │  │
│  └────────────────┘                           └────────┬───────────┘  │
│                                                        │ read         │
└────────────────────────────────────────────────────────┼──────────────┘
                                                         │
                        ┌────────────────────────────────▼──────────────┐
                        │          Execution Events Gateway             │
                        │                                               │
                        │  ┌─────────────┐   ┌──────────────────────┐  │
                        │  │ Event        │   │ Broadcast            │  │
                        │  │ Listener     ├──▶│ Channel              │  │
                        │  │ (ring reader)│   │ (fan-out to clients) │  │
                        │  └─────────────┘   └──────┬───────────────┘  │
                        │                           │                   │
                        │  ┌────────────────────────▼────────────────┐  │
                        │  │            Per-Client Pipeline           │  │
                        │  │                                          │  │
                        │  │  subscription filter → backpressure     │  │
                        │  │  → bounded channel → WebSocket send     │  │
                        │  └──────────────────────────────────────────┘  │
                        │                                               │
                        │  ┌──────────────────────┐  ┌──────────────┐  │
                        │  │ Ring Buffer           │  │ REST API     │  │
                        │  │ (100K pre-serialized  │  │ /v1/tps      │  │
                        │  │  messages for resume) │  │ /v1/status   │  │
                        │  └──────────────────────┘  │ /v1/contention│  │
                        │                             └──────────────┘  │
                        └───────────────────────────────────────────────┘
                                  │
                    ┌─────────────┼─────────────────┐
                    │             │                  │
              ┌─────▼─────┐ ┌────▼─────┐ ┌─────────▼───────┐
              │ TS SDK    │ │ Python   │ │ Webhook Relay   │
              │ Client    │ │ Client   │ │ (sidecar)       │
              └───────────┘ └──────────┘ └────────┬────────┘
                                                   │ HTTP POST
                                            ┌──────▼──────┐
                                            │ Your Service│
                                            └─────────────┘
```

## Data Flow

### 1. Event Ring → Gateway

The Monad validator writes execution events into a **memory-mapped ring buffer** backed by hugepages. The gateway's **Event Listener** thread polls this ring at microsecond granularity, converting raw C structs into typed Rust structs (`SerializableExecEvent`).

Key properties:
- **Zero-copy read**: The ring is mmap'd — no kernel transitions for reads
- **No backpressure upstream**: The ring overwrites old entries when full. The gateway must keep up.
- **Ordering**: Events arrive in execution order within a block

### 2. Enrichment & Broadcast

Each event is enriched with metadata:
- **Block lifecycle tracking**: The gateway maintains a state machine per block, tracking stage transitions (Proposed → Voted → Finalized → Verified)
- **Contention analysis**: Per-block storage slot contention metrics computed on `BlockEnd`
- **TPS calculation**: Rolling 2.5-block window (~1 second at 400ms block time)

Enriched events are assigned a monotonic `server_seqno`, pre-serialized to JSON, stored in the ring buffer for resume, and broadcast to all subscribers.

### 3. Per-Client Pipeline

Each WebSocket connection has its own:
- **Subscription filter**: Which event types, field-level filters, minimum stage
- **Bounded send channel**: 4,096 message capacity
- **Backpressure handling**: Messages dropped when channel full; client disconnected after 10,000 drops

## Key Design Decisions

### Pre-serialized Ring Buffer

The resume ring buffer stores **pre-serialized JSON strings**, not structured data. Benefits:
- Resume replay is zero-cost (memcpy, no re-serialization)
- Each entry is ~200–800 bytes (typical event JSON)
- 100,000 entries ≈ 20–80 MB memory

### Monotonic Server Seqno

Two sequence number systems exist:
- **Event ring seqno** (`seqno` field inside events): From the validator's ring buffer, can wrap
- **Server seqno** (`server_seqno` in wire messages): Gateway-assigned, strictly monotonic, never wraps

Clients use `server_seqno` for cursor resume. The event ring `seqno` is informational.

### Channel-Based Architecture

Instead of a single firehose, the gateway offers purpose-built channels:

| Channel | Target Audience | Volume |
|---------|----------------|--------|
| `/v1/ws` | Full firehose consumers | Very high |
| `/v1/ws/blocks` | Block explorers, dashboards | Low |
| `/v1/ws/txs` | MEV bots, indexers | High |
| `/v1/ws/contention` | Analytics, parallel optimization | Low (1/block) |
| `/v1/ws/lifecycle` | Infrastructure monitoring | Very low |

Each channel has a default subscription that clients can further narrow.

---

# Event Model

## Event Envelope

Every execution event carries:

```json
{
  "event_name": "BlockFinalized",
  "block_number": 56147820,
  "txn_idx": null,
  "txn_hash": null,
  "commit_stage": "Finalized",
  "payload": { "type": "BlockFinalized", "block_id": "0x...", "block_number": 56147820 },
  "seqno": 9876543210,
  "timestamp_ns": 1708345678000000000
}
```

The `commit_stage` field reflects the block's **current** consensus stage at the time the event was sent. This means:
- An event originally emitted at `Proposed` stage will be re-tagged as `Finalized` if the client's `min_stage` filter requires it
- Stage-aware filtering is applied server-side, not retroactively

## Event Categories

| Category | Events | Frequency |
|----------|--------|-----------|
| Block lifecycle | `BlockStart`, `BlockEnd`, `BlockQC`, `BlockFinalized`, `BlockVerified`, `BlockReject` | 1 per block per stage |
| Transaction lifecycle | `TxnHeaderStart`, `TxnEvmOutput`, `TxnEnd`, `TxnReject` | 1 per txn per event |
| EVM details | `TxnLog`, `TxnCallFrame`, `NativeTransfer` | Varies per txn |
| State access | `AccountAccess`, `StorageAccess`, `AccountAccessListHeader` | Very high |
| Performance | `BlockPerfEvmEnter/Exit`, `TxnPerfEvmEnter/Exit` | 2 per block/txn |
| Errors | `RecordError`, `EvmError` | Rare |

---

# Stage Model (MonadBFT Consensus)

```
                    ┌──────────┐
         ┌─────────│ Proposed  │─────────┐
         │         └────┬─────┘          │
         │              │                │
         │         ┌────▼─────┐          │
         │         │  Voted   │          │
         │         │  (QC)    │──────────┤
         │         └────┬─────┘          │
         │              │                │
         │         ┌────▼──────┐         │
         │         │ Finalized │         │
         │         │ (commit)  │─────────┤
         │         └────┬──────┘         │
         │              │                │
         │         ┌────▼──────┐    ┌────▼─────┐
         │         │ Verified  │    │ Rejected │
         │         │ (terminal)│    │ (terminal)│
         │         └───────────┘    └──────────┘
         │                               ▲
         └───────────────────────────────┘
              (rejected from any stage)
```

### Stage Semantics

| Stage | Trigger Event | Typical Latency | Finality Guarantee |
|-------|--------------|-----------------|-------------------|
| **Proposed** | `BlockStart` | t=0 | None — speculative |
| **Voted** | `BlockQC` | ~400ms | Speculative finality (2/3+ validators voted) |
| **Finalized** | `BlockFinalized` | ~800ms | Full finality — irreversible |
| **Verified** | `BlockVerified` | After finalization | State root verified (terminal) |
| **Rejected** | `BlockReject` | Varies | Block dropped (terminal) |

### Stage Filtering (`min_stage`)

The `min_stage` subscription option gates event delivery:

```json
{"subscribe": {"events": ["TxnLog"], "min_stage": "Finalized"}}
```

With `min_stage: "Finalized"`:
- Events from Proposed/Voted blocks → **held back**
- Events from Finalized/Verified blocks → **delivered immediately**
- Events from Rejected blocks → **never delivered**

This is critical for applications that need finality guarantees (e.g., payment processors, bridge relayers).

---

# Resume Semantics

## The Resume Protocol

```
Client                                     Gateway
  │                                          │
  │  WS connect ?resume_from=12345           │
  │─────────────────────────────────────────▶│
  │                                          │ Check ring buffer:
  │                                          │ seqno 12345 in range?
  │                                          │
  │  {"server_seqno":0, "Resume":            │
  │   {"mode":"resume"}}                     │
  │◀─────────────────────────────────────────│
  │                                          │
  │  {seqno:12346, "Events":[...]}           │ Replay from ring buffer
  │◀─────────────────────────────────────────│ (pre-serialized, zero-cost)
  │  {seqno:12347, "Events":[...]}           │
  │◀─────────────────────────────────────────│
  │  ...                                     │
  │                                          │
  │  {seqno:54321, "TPS": 2400}             │ Live stream begins
  │◀─────────────────────────────────────────│
```

### Resume Modes

| Mode | Condition | Behavior |
|------|-----------|----------|
| `"resume"` | `resume_from` seqno is within the ring buffer window | Lossless replay of all missed messages |
| `"snapshot"` | Cursor too old, or fresh connect without `resume_from` | Current state snapshot (TPS, contention, active lifecycles) |

### Ring Buffer Window

The ring buffer holds the last **100,000 pre-serialized messages**. At typical throughput:
- ~5,000 events/second → ~20 seconds of history
- ~1,000 events/second → ~100 seconds of history

Check the window via REST before connecting:

```bash
curl http://host:8443/v1/status
# { "oldest_seqno": 4321, "newest_seqno": 54321 }
```

If `your_cursor >= oldest_seqno`, resume will succeed.

### Resume ACK Validation

The SDK provides `waitForResume()` to validate the resume mode:

```ts
await client.connect();
const { mode } = await client.waitForResume(5000);

if (mode === "snapshot") {
  // Gap detected — need to rebuild state from REST APIs or snapshot data
  const contention = await GatewayClient.fetchContention(url);
  const lifecycle = await GatewayClient.fetchLifecycle(url);
}
```

### Idempotency

Resume replay sends the exact same JSON bytes that were originally sent. Clients receiving replayed messages will see the same `server_seqno` values, making deduplication trivial — just skip messages with `seqno <= last_processed`.

---

# Backpressure Semantics

## The Problem

The gateway broadcasts to potentially hundreds of clients. A single slow consumer must not:
1. Block the broadcast to other clients
2. Cause unbounded memory growth on the server
3. Slow down the event listener (which must keep up with the ring)

## The Solution: Bounded Channel + Drop + Disconnect

```
                    ┌─────────────────────────────────┐
  Broadcast ──────▶ │  Per-client bounded channel      │ ──────▶ WebSocket send
                    │  capacity: 4,096 messages        │
                    └─────────────────────────────────┘
                              │
                              │ (channel full)
                              ▼
                    ┌─────────────────────────────────┐
                    │  DROP message + increment counter │
                    │                                   │
                    │  Every 1,000 drops: log warning   │
                    │  After 10,000 drops: DISCONNECT   │
                    └─────────────────────────────────┘
```

### Constants

| Parameter | Value | Purpose |
|-----------|-------|---------|
| `CLIENT_SEND_BUFFER` | 4,096 | Per-client channel capacity |
| `SLOW_CLIENT_DROP_LIMIT` | 10,000 | Cumulative drops before disconnect |
| Warning interval | Every 1,000 drops | Logged server-side |

### What Clients Should Do

1. **Process messages fast**: Don't do heavy work in the message handler. Push to a queue.
2. **Track drops**: If you see gaps in `server_seqno`, you're being backpressured.
3. **Use `resume_from`**: After disconnect, reconnect with your last seqno to replay missed messages.
4. **Subscribe narrowly**: Only subscribe to event types you need. Use field filters.

### Drop Detection

Clients can detect drops by checking `server_seqno` gaps:

```ts
let lastSeqno = 0;
client.on("events", (events) => {
  const currentSeqno = client.serverSeqno!;
  if (lastSeqno > 0 && currentSeqno > lastSeqno + 1) {
    console.warn(`Gap detected: ${lastSeqno} → ${currentSeqno}`);
  }
  lastSeqno = currentSeqno;
});
```

---

# SLA Expectations

## Availability

| Component | Target | Notes |
|-----------|--------|-------|
| Gateway process | 99.9% | Auto-restarts via Docker/systemd on crash |
| WebSocket endpoint | 99.9% | Health check at `/health` triggers restart if stale |
| Resume buffer | Best effort | Ring buffer is in-memory, lost on restart |

## Latency

| Metric | Typical | Maximum |
|--------|---------|---------|
| Event ring → first client | < 1ms | 5ms |
| Proposed → Voted | ~400ms | Block-dependent |
| Proposed → Finalized | ~800ms | Block-dependent |
| Resume replay (100K messages) | < 100ms | 500ms |

## Throughput

| Metric | Value |
|--------|-------|
| Max events/second | ~50,000+ (event ring throughput) |
| Max concurrent WebSocket clients | Limited by `MAX_CONNECTIONS_PER_IP` (10/IP) and system resources |
| Broadcast fan-out | O(n) clients, non-blocking per client |

## Guarantees

| Property | Guarantee |
|----------|-----------|
| Ordering | Events within a block are ordered. Cross-block ordering is by `server_seqno`. |
| Delivery | At-most-once per connection. Use `resume_from` for at-least-once across reconnections. |
| Deduplication | `server_seqno` is unique and monotonic. Clients can deduplicate trivially. |
| Backpressure | Slow clients are dropped, not the broadcast pipeline. |
| Data loss on restart | Resume buffer is in-memory. Gateway restart = buffer reset. Clients get `"snapshot"` mode. |
