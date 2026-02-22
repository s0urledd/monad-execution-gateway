# Introducing Monad Execution Events Gateway: Real-Time EVM Visibility That JSON-RPC Can't Give You

Every blockchain node has a black box problem. You submit a transaction, wait for a receipt, and hope for the best. If something goes wrong — a reverted call, unexpected gas usage, a frontrun — you reconstruct what happened from logs *after the fact*.

What if you could watch the EVM execute in real time? Not after the block is finalized. Not from a receipt. **As the validator processes each transaction, instruction by instruction.**

That's what the Monad Execution Events Gateway does.

---

## The Problem

Standard Ethereum infrastructure gives you three observation points:

1. **`eth_subscribe("newHeads")`** — A block was produced. Here's the header.
2. **`eth_subscribe("logs")`** — Some contract emitted an event.
3. **`eth_getLogs`** — Poll for historical log entries.

That's it. You're looking at the blockchain through a keyhole.

You can't see:
- Which storage slots a transaction touched
- Which internal `CALL`/`DELEGATECALL` frames were created
- How long the EVM spent executing each transaction
- Whether a block is still speculative (`Proposed`) or irreversible (`Finalized`)
- Which contracts are causing storage slot contention under parallel execution
- Real-time TPS without polling

For indexers, this means polling. For MEV searchers, this means guessing. For block explorers, this means reconstructing history from `debug_traceBlock` calls — after the fact, one block at a time.

## The Solution: A Shared-Memory Streaming Sidecar

The Monad Execution Events Gateway is a standalone process that runs alongside a Monad validator. It doesn't use JSON-RPC. It doesn't poll. Instead, it reads directly from the validator's **execution event ring** — a memory-mapped ring buffer backed by hugepages.

```
┌────────────────────────────────────────────────────┐
│                  Monad Validator                    │
│                                                    │
│  Execution Engine ──mmap (hugepages)──► Event Ring │
└──────────────────────────────────────────┬─────────┘
                                           │ shared memory (zero-copy read)
                                           ▼
┌──────────────────────────────────────────────────────┐
│              Execution Events Gateway                │
│                                                      │
│  Ring Reader → Enrichment → Broadcast → Per-Client   │
│                                         Filtering    │
│                                         & Delivery   │
└──────────────────────────┬───────────────────────────┘
                           │ WebSocket
              ┌────────────┼───────────────┐
              ▼            ▼               ▼
          Your Bot    Your Indexer    Your Dashboard
```

The validator writes raw execution events to shared memory. The gateway reads them at microsecond granularity — no kernel transitions, no serialization overhead, no network hop. Each event is enriched with consensus stage metadata, assigned a monotonic sequence number, and pushed to connected clients over WebSocket.

**Zero-copy from validator to wire.**

---

## What Can You See?

24 distinct event types across 6 categories. Here's what becomes visible:

### Block Lifecycle — From Proposal to Finality

Every block passes through Monad's BFT consensus stages:

```
Proposed → Voted (QC) → Finalized → Verified
                                         │
            (any stage) ──────────► Rejected
```

The gateway tracks each transition with millisecond timing:

```json
{
  "server_seqno": 46,
  "Lifecycle": {
    "block_number": 56147820,
    "from_stage": "Proposed",
    "to_stage": "Finalized",
    "time_in_previous_stage_ms": 412.5,
    "block_age_ms": 812.3,
    "txn_count": 150,
    "gas_used": 12500000
  }
}
```

You know the exact millisecond a block went from speculative to irreversible. No other interface gives you this.

### Transaction Internals — Every Frame, Every Access

For every transaction in a block, you see:

- **`TxnHeaderStart`** — Sender, receiver, value, nonce, gas limit, calldata, full signature
- **`TxnLog`** — Each log emission as it happens (address, topics, data)
- **`TxnCallFrame`** — Internal CALL/DELEGATECALL/STATICCALL frames with depth, caller, target, value, input, output
- **`AccountAccess`** / **`StorageAccess`** — Every account and storage slot the transaction reads
- **`TxnEvmOutput`** — Final result: success/revert, gas used, log count
- **`TxnEnd`** — Transaction complete

This isn't a post-hoc trace reconstruction. These are the actual execution events as they're produced by the EVM, streamed to you in real time.

### Contention Analytics — Parallel Execution Visibility

Monad executes transactions in parallel. When two transactions touch the same storage slot, one gets re-executed. The gateway computes per-block contention metrics:

```json
{
  "server_seqno": 44,
  "ContentionData": {
    "block_number": 56147820,
    "parallel_efficiency_pct": 87.14,
    "contention_ratio": 0.0275,
    "total_unique_slots": 1523,
    "contended_slot_count": 42,
    "top_contended_slots": [...],
    "top_contended_contracts": [...],
    "contract_edges": [...]
  }
}
```

`parallel_efficiency_pct` tells you how much of the block's execution time benefited from parallelism. `top_contended_contracts` shows which contracts are serialization bottlenecks. `contract_edges` reveals co-access patterns — which contracts touch the same state.

This data doesn't exist anywhere else. It's computed live, per block, and streamed to subscribers.

---

## How Is This Different?

### vs Ethereum JSON-RPC / WebSocket Subscriptions

| | JSON-RPC | This Gateway |
|---|----------|-------------|
| **Data source** | Finalized chain state | Validator's in-process event ring (shared memory) |
| **Granularity** | Block headers, receipts, logs | 24 event types: call frames, storage reads, EVM timing |
| **Consensus stages** | None — you see blocks after finality | Proposed → Voted → Finalized → Verified (with ms timing) |
| **Contention data** | Not available | Per-block parallel efficiency, contended slots, contract hotspots |
| **Filtering** | Address + topic filters on logs | Event type + field-level + stage-aware (`min_stage: "Finalized"`) |
| **Reconnection** | Client must re-query and deduplicate | Server-side cursor resume from 100K-entry ring buffer |
| **Latency** | Block-level (after production) | Sub-block (as each transaction executes) |

JSON-RPC was designed for querying settled state. This gateway streams execution **as it happens**.

### vs Monad Node RPC

Monad nodes expose an Ethereum-compatible JSON-RPC interface. The execution event ring is an internal data path that the node doesn't expose over the network.

This gateway bridges that gap. It reads the event ring from shared memory and streams it over WebSocket. They're complementary:

- **Node RPC**: Submit transactions, query state, call contracts
- **Gateway**: Observe execution, track consensus, analyze contention

You need both. RPC for interaction, gateway for visibility.

### vs Third-Party Indexers (The Graph, Goldsky, etc.)

Third-party indexers focus on indexed historical data. You define a subgraph, they index it, you query via GraphQL.

This gateway is different in kind:

- **Real-time push**, not indexed pull
- **Pre-finality data** — you see events from `Proposed` blocks
- **No indexing lag** — events arrive in sub-milliseconds
- **EVM internals** — call frames, storage access, contention — not just logs
- **Self-hosted** — runs next to your validator, no third-party dependency

---

## Who Should Use This?

### MEV Researchers and Searchers

You get every transaction's storage access pattern and internal call frames **as the block is being built**. You can see contention hotspots and parallel execution dynamics before they're publicly visible.

Subscribe to `/v1/ws/txs` for transaction events, or `/v1/ws` for the full firehose.

### Indexers and Data Pipelines

Replace `eth_getLogs` polling with a push-based stream. The cursor resume protocol (`?resume_from=<seqno>`) gives you lossless delivery across reconnections — the server replays missed messages from a 100K-entry ring buffer.

Stage-aware filtering lets you gate on finality:

```json
{"subscribe": {"events": ["TxnLog"], "min_stage": "Finalized"}}
```

Only finalized events. No speculative data. No reorg handling on your side.

### Block Explorers and Dashboards

Lifecycle timing (proposal-to-finality in milliseconds), live TPS, parallel efficiency scores — all push-based, all real time. No polling, no `setTimeout`.

Connect to `/v1/ws/lifecycle` for stage transitions, `/v1/ws/blocks` for block events + TPS.

### Protocol Developers

Contention analytics reveal which contracts and storage slots cause re-execution under parallel execution. If you're optimizing a smart contract for Monad, this is the feedback loop you need.

`/v1/ws/contention` gives you per-block contention data with top contended slots, contract co-access edges, and parallel efficiency percentages.

### Infrastructure Operators

50+ Prometheus metrics at `/metrics`. Health checks at `/health` with auto-exit on event stalls (triggers container restart). Configurable heartbeat intervals. Degraded-state detection via `/v1/status`.

---

## Getting Started

### 1. Run the Gateway

```bash
# Docker (recommended — runs alongside your Monad node)
docker compose up -d

# Or native
cd gateway && ./build.sh --run
```

### 2. Connect

```bash
# Watch block lifecycle transitions
websocat ws://localhost:8443/v1/ws/lifecycle

# Check gateway status
curl http://localhost:8443/v1/status

# Full firehose
websocat ws://localhost:8443/v1/ws
```

### 3. Use an SDK

**TypeScript:**

```typescript
import { GatewayClient } from "monad-execution-events";

const client = new GatewayClient({
  url: "ws://localhost:8443",
  channel: "lifecycle",
});

client.on("lifecycle", (update) => {
  console.log(
    `Block ${update.block_number}: ${update.to_stage} (${update.block_age_ms}ms)`
  );
});

await client.connect();
```

**Python:**

```python
from monad_execution_events import GatewayClient, GatewayClientOptions, Channel

client = GatewayClient(GatewayClientOptions(
    url="ws://localhost:8443",
    channel=Channel.LIFECYCLE,
))

@client.on("lifecycle")
def on_block(update):
    print(f"Block {update.block_number}: {update.to_stage.value}")

await client.connect()
await client.listen_forever()
```

SDKs handle auto-reconnect with exponential backoff, cursor resume, heartbeat detection, and typed events out of the box.

### 4. Resume After Disconnect

Every message has a monotonic `server_seqno`. Store the last one you processed. On reconnect:

```
ws://localhost:8443/v1/ws?resume_from=42
```

The server replays everything you missed from its 100K-entry ring buffer. If your cursor is too old, you get a `"snapshot"` instead of `"resume"` — the SDKs detect this automatically.

---

## Architecture at a Glance

```
Monad Validator
  └── Execution Engine
        └── writes to ──► Event Ring (mmap'd hugepages, shared memory)
                                │
                                │ zero-copy read
                                ▼
                          Gateway Process
                            ├── Event Listener (ring reader thread)
                            ├── Enrichment Pipeline
                            │     ├── Block lifecycle state machine
                            │     ├── Contention analytics
                            │     ├── TPS calculation
                            │     └── Top-K access tracking
                            ├── Broadcast Channel (1M capacity, fan-out)
                            ├── Per-Client Pipeline
                            │     ├── Subscription filter
                            │     ├── Stage-aware gating
                            │     ├── Bounded channel (4,096)
                            │     └── Backpressure (drop + disconnect)
                            ├── Resume Ring Buffer (100K pre-serialized entries)
                            ├── REST API (/v1/tps, /v1/contention, /v1/status)
                            └── Prometheus Metrics (/metrics, 50+ series)
```

Key design properties:

- **Zero-copy from ring to broadcast** — mmap'd hugepages, no kernel transitions
- **Pre-serialized replay** — ring buffer stores JSON strings, resume is memcpy
- **Per-client isolation** — a slow client gets dropped, never the pipeline
- **No persistence** — in-memory only, gateway restart = clean slate
- **No rate limits** — designed for trusted/operator-controlled environments

---

## Endpoints Reference

| Endpoint | Type | Description |
|----------|------|-------------|
| `/v1/ws` | WebSocket | Full firehose — all events + all metrics |
| `/v1/ws/blocks` | WebSocket | Block events + TPS + Lifecycle |
| `/v1/ws/txs` | WebSocket | Transaction events (MEV, indexing) |
| `/v1/ws/contention` | WebSocket | Per-block contention analytics |
| `/v1/ws/lifecycle` | WebSocket | Block stage transitions only |
| `/v1/tps` | REST | Current TPS snapshot |
| `/v1/contention` | REST | Latest per-block contention data |
| `/v1/blocks/lifecycle` | REST | All tracked block lifecycles |
| `/v1/blocks/:number/lifecycle` | REST | Single block lifecycle timeline |
| `/v1/status` | REST | Gateway health + resume window info |
| `/health` | REST | Health probe (exits on 30s event stall) |
| `/metrics` | REST | Prometheus metrics (50+ series) |

All WebSocket endpoints accept `?resume_from=<server_seqno>` for lossless reconnection.

---

## Open Source

The gateway is MIT-licensed. Rust-based, single binary, minimal dependencies.

- **Gateway**: Rust (Axum + Tokio + serde_json)
- **TypeScript SDK**: `npm install monad-execution-events`
- **Python SDK**: `pip install monad-execution-events`
- **Webhook Relay**: Node.js sidecar for HTTP endpoint forwarding

GitHub: [monad-execution-gateway](https://github.com/monad-labs/monad-execution-gateway)

---

*The Monad Execution Events Gateway is built by [Huginn Tech](https://huginn.tech) for the Monad ecosystem. It gives developers, researchers, and operators a window into EVM execution that has never existed before on any EVM chain.*
