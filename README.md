# Monad Execution Events Gateway

Real-time execution event streaming from a Monad full node. EVM-internal visibility not available through standard JSON-RPC.

---

## What is this?

A standalone WebSocket gateway that reads raw execution events directly from a Monad validator's shared-memory ring buffer and streams them to clients in real time. It runs as a sidecar next to a Monad full node — no RPC calls, no polling, no middleware.

The validator writes events to an mmap'd hugepage ring buffer. This gateway reads that ring at microsecond granularity, enriches each event with consensus stage metadata, and fans them out over WebSocket channels with per-client filtering, backpressure management, and a lossless resume protocol.

## What can you see?

24 distinct execution event types that are invisible to standard interfaces:

| Category | Events | What you learn |
|----------|--------|----------------|
| **Block lifecycle** | `BlockStart`, `BlockEnd`, `BlockQC`, `BlockFinalized`, `BlockVerified`, `BlockReject` | Full block progression from proposal through consensus to finality |
| **Transaction lifecycle** | `TxnHeaderStart`, `TxnHeaderEnd`, `TxnEvmOutput`, `TxnEnd`, `TxnReject` | Every transaction from header parsing to final receipt |
| **EVM internals** | `TxnLog`, `TxnCallFrame`, `NativeTransfer` | Log emissions, internal call traces, native ETH transfers — as they happen |
| **State access** | `AccountAccess`, `StorageAccess`, `AccountAccessListHeader` | Which accounts and storage slots each transaction reads |
| **Contention analytics** | `ContentionData` (computed) | Per-block slot contention ratio, parallel efficiency, top contended contracts |
| **Performance** | `BlockPerfEvmEnter/Exit`, `TxnPerfEvmEnter/Exit` | Execution timing at block and transaction granularity |

Plus computed metrics: **TPS** (rolling window), **Top-K accessed accounts/slots** (Space-Saving algorithm), and **block lifecycle transitions** with millisecond-precision timing.

## How is this different from standard RPC / WSS?

### vs Ethereum JSON-RPC (`eth_subscribe`, `eth_getLogs`)

| | JSON-RPC | This Gateway |
|---|----------|-------------|
| **Data source** | Reads from finalized chain state | Reads from validator's in-process event ring (shared memory) |
| **Event granularity** | Block headers, transaction receipts, logs | 24 event types including call frames, storage access patterns, EVM entry/exit timing |
| **Consensus visibility** | None — you see blocks after finality | Full stage tracking: Proposed → Voted → Finalized → Verified (with ms timing) |
| **Contention data** | Not available | Per-block parallel efficiency, contended slot counts, top contended contracts |
| **Filtering** | `eth_getLogs` with address/topic filters | Event name + field-level filters + stage-aware gating (`min_stage: "Finalized"`) |
| **Reconnection** | Client must re-query and deduplicate | Server-side cursor resume: `?resume_from=<seqno>` replays from 100K-entry ring buffer |
| **Latency** | Block-granularity (events arrive after block production) | Sub-block: events stream as the validator executes each transaction |

JSON-RPC is designed for querying settled state. This gateway streams execution **as it happens**, before the block even reaches consensus.

### vs Monade (Monad Node RPC)

Monad nodes expose an Ethereum-compatible JSON-RPC interface. The execution event ring is a low-level internal data path that the node itself does not expose over the network.

This gateway is the bridge: it reads the event ring from shared memory and makes that data accessible over WebSocket. Think of it as a dedicated streaming sidecar for the validator's execution pipeline.

| | Monad Node RPC | This Gateway |
|---|----------------|-------------|
| **Interface** | JSON-RPC (request/response) | WebSocket (server-push streaming) |
| **Scope** | Standard Ethereum methods (`eth_*`, `debug_*`) | Execution events, lifecycle, contention, access patterns |
| **Deployment** | Built into the node | Separate process, reads shared memory |
| **Use case** | Transaction submission, state queries | Real-time monitoring, analytics, MEV research, indexing |

They are complementary. RPC for reads and writes; this gateway for observability.

## Who is this for?

- **MEV researchers / searchers** — See every transaction's storage access pattern and internal call frames in real time. Identify contention hotspots before they become public.
- **Indexers / data pipelines** — Stream all execution events with lossless resume instead of polling `eth_getLogs`. Stage-aware filtering lets you gate on finality.
- **Block explorers / dashboards** — Block lifecycle timing (proposal-to-finality in ms), TPS metrics, parallel efficiency scores — all push-based.
- **Protocol developers** — Contention analytics reveal which contracts and storage slots cause re-execution in Monad's parallel execution engine.
- **Infrastructure operators** — Health checks, Prometheus metrics (50+ counters/histograms/gauges), degraded-state detection, configurable heartbeat.
- **Wallet / DApp backends** — Subscribe to specific addresses or log topics with field-level filters. Only receive finalized events with `min_stage`.

---

## Quick Start

```bash
# Docker (recommended)
docker compose up -d

# Native
cd gateway && ./build.sh --run
```

Connect:

```bash
websocat ws://localhost:8443/v1/ws/lifecycle
curl http://localhost:8443/v1/status
```

## Endpoints

| Endpoint | Description |
|----------|-------------|
| `/v1/ws` | All events (firehose) |
| `/v1/ws/blocks` | Block events + TPS |
| `/v1/ws/txs` | Transaction events |
| `/v1/ws/contention` | Contention analytics |
| `/v1/ws/lifecycle` | Block stage transitions |
| `/v1/tps` | REST: current TPS |
| `/v1/contention` | REST: contention data |
| `/v1/status` | REST: gateway status |
| `/v1/blocks/lifecycle` | REST: block lifecycles |
| `/health` | Health check |

All WebSocket endpoints accept `?resume_from=<server_seqno>` for lossless reconnect.

## TypeScript SDK

```bash
npm install monad-execution-events
```

```typescript
import { GatewayClient } from "monad-execution-events";

const client = new GatewayClient({
  url: "ws://localhost:8443",
  channel: "lifecycle",
});

client.on("lifecycle", (update) => {
  console.log(`Block ${update.block_number}: ${update.to_stage} (${update.block_age_ms}ms)`);
});

await client.connect();
```

Features: auto-reconnect with exponential backoff, cursor resume, heartbeat detection, typed events, `waitForResume()` ACK validation.

## Python SDK

```bash
pip install monad-execution-events
```

```python
import asyncio
from monad_execution_events import GatewayClient, GatewayClientOptions, Channel

async def main():
    client = GatewayClient(GatewayClientOptions(
        url="ws://localhost:8443",
        channel=Channel.LIFECYCLE,
    ))

    @client.on("lifecycle")
    def on_block(update):
        print(f"Block {update.block_number}: {update.to_stage.value}")

    await client.connect()
    await client.listen_forever()

asyncio.run(main())
```

## Webhook Relay

Forward gateway events to HTTP endpoints (at-least-once delivery):

```bash
GATEWAY_URL=ws://localhost:8443 \
WEBHOOK_URLS=https://your-service/hooks/blocks \
node webhook-relay/dist/index.js
```

## Cursor Resume

Every message carries a monotonic `server_seqno`. On reconnect, pass `?resume_from=<seqno>` to replay missed messages from a 100K-entry ring buffer. SDKs handle this automatically.

```
Connect:    ws://host:8443/v1/ws
            <- {"server_seqno":0, "Resume":{"mode":"snapshot"}}
            <- {"server_seqno":5, "Events":[...]}
            ...disconnect at seqno 42...

Reconnect:  ws://host:8443/v1/ws?resume_from=42
            <- {"server_seqno":42, "Resume":{"mode":"resume"}}
            <- {"server_seqno":43, ...}  // picks up where you left off
```

## Docs

- [Architecture](docs/ARCHITECTURE.md) — system design, stage model, resume/backpressure semantics
- [Wire Protocol](docs/wire.md) — exact frame format, message types, envelope structure
- [Behavioral Spec](docs/spec.md) — normative contract: ordering, lifecycle, backpressure, resume
- [SLO Definitions](docs/slo.md) — observed performance baselines with reference hardware
- [API Reference](docs/API.md) — endpoint catalog, subscription protocol
- [Events Reference](docs/EVENTS.md) — all event types with field descriptions
- [Deployment](docs/DEPLOYMENT.md) — Docker, native build, systemd

## License

MIT
