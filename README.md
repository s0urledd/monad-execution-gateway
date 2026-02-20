# Monad Execution Events Gateway

Community-maintained public Execution Events Gateway for Monad builders. Real-time execution event streaming from a Monad validator node with **EVM-internal visibility** not available through standard Ethereum JSON-RPC.

## What This Streams

| Data | Standard RPC | This Gateway |
|------|:---:|:---:|
| Block headers, logs, tx receipts | yes | yes |
| Block lifecycle stages (Proposed/Voted/Finalized/Verified) | — | yes |
| Per-event commit stage (finality confidence) | — | yes |
| Per-tx storage slot reads/writes | — | yes |
| Account access patterns | — | yes |
| Parallel execution efficiency | — | yes |
| Storage contention (hot slots) | — | yes |
| Contract co-access graphs | — | yes |
| Real-time TPS (execution-level) | — | yes |
| Internal call frames | — | yes |

**Data source**: Monad's execution engine writes events to a hugepage-backed memory-mapped ring buffer. The gateway reads from this ring with zero-copy, then streams JSON over WebSocket.

## Quick Start

### Docker

```bash
git clone https://github.com/s0urledd/monad-execution-gateway.git
cd monad-execution-gateway
docker compose up -d
```

### Native

```bash
cd gateway
./build.sh --run
```

See [Deployment Guide](docs/DEPLOYMENT.md) for detailed setup instructions.

### Connect

```bash
# All events
websocat ws://your-validator:8443/v1/ws

# Block lifecycle only (Proposed -> Voted -> Finalized -> Verified)
websocat ws://your-validator:8443/v1/ws/lifecycle

# Blocks + TPS
websocat ws://your-validator:8443/v1/ws/blocks

# REST snapshots
curl http://your-validator:8443/v1/tps
curl http://your-validator:8443/v1/status
curl http://your-validator:8443/v1/blocks/lifecycle
```

### TypeScript SDK

```typescript
import { GatewayClient } from "@monad-labs/execution-events";

const client = new GatewayClient({
  url: "ws://your-validator:8443",
  channel: "lifecycle",   // "all" | "blocks" | "txs" | "contention" | "lifecycle"
});

// First frame tells you the resume mode
client.on("resume", (info) => {
  console.log(`Connected in ${info.mode} mode`);   // "resume" or "snapshot"
});

// Watch blocks progress through consensus stages
client.on("lifecycle", (update) => {
  console.log(`Block ${update.block_number}: ${update.to_stage} (${update.block_age_ms}ms)`);
});

// Every event carries its block's current commit stage
client.on("event", (event) => {
  console.log(event.event_name, event.commit_stage, event.payload);
});

client.on("tps", (tps) => console.log("TPS:", tps));

await client.connect();

// Only receive events from finalized blocks
client.subscribe({ events: ["TxnLog"], min_stage: "Finalized" });
```

```typescript
// REST helpers (no WebSocket needed)
const { tps } = await GatewayClient.fetchTPS("http://your-validator:8443");
const status = await GatewayClient.fetchStatus("http://your-validator:8443");
const lifecycle = await GatewayClient.fetchLifecycle("http://your-validator:8443");
```

## API Endpoints

All endpoints served on a single port (default `8443`):

| Endpoint | Type | Description |
|----------|------|-------------|
| `/v1/ws` | WebSocket | All events + all computed metrics |
| `/v1/ws/blocks` | WebSocket | Block lifecycle events + TPS |
| `/v1/ws/txs` | WebSocket | Transaction events only |
| `/v1/ws/contention` | WebSocket | Contention data only |
| `/v1/ws/lifecycle` | WebSocket | Block stage transitions only |
| `/v1/tps` | REST | Current TPS snapshot |
| `/v1/contention` | REST | Latest per-block contention data |
| `/v1/blocks/lifecycle` | REST | Lifecycle summaries for recent blocks |
| `/v1/blocks/:number/lifecycle` | REST | Full lifecycle for a specific block |
| `/v1/status` | REST | Gateway status + cursor resume window |
| `/health` | REST | Health check |

All WebSocket endpoints accept `?resume_from=<server_seqno>` for cursor-based reconnect (see below).

## Cursor Resume

Every wire message carries a `server_seqno` — a monotonic sequence number. On reconnect, the client passes `?resume_from=<last_seen_seqno>` and the server replays only the missed messages from an in-memory ring buffer (100K entries). No duplicate data, no full re-replay.

```
First connect:          ws://host:8443/v1/ws
                        <- {"server_seqno":0, "Resume":{"mode":"snapshot"}}
                        <- {"server_seqno":1, "TPS":2450}
                        <- {"server_seqno":5, "Events":[...]}
                        ...client disconnects at seqno 42...

Reconnect:              ws://host:8443/v1/ws?resume_from=42
                        <- {"server_seqno":42, "Resume":{"mode":"resume"}}
                        <- {"server_seqno":43, ...}   // picks up right where you left off
```

If the cursor is too old (evicted from the buffer), the server falls back to a snapshot replay and sends `{"mode":"snapshot"}`.

The SDK handles this automatically — `lastServerSeqno` is tracked internally and `?resume_from` is appended on every reconnect.

Check the replay window via REST:

```bash
curl http://your-validator:8443/v1/status
# {"server_seqno":54321, "oldest_seqno":4321, "newest_seqno":54321, ...}
```

## Backpressure

Each WebSocket client gets a bounded send buffer (4096 messages). If a client falls behind:

1. Messages are dropped (not queued indefinitely)
2. Warning logged every 1000 drops
3. Client disconnected after 10,000 drops

This prevents a single slow consumer from exhausting server memory.

## Architecture

```
Monad Validator Node
        |
        v
+---------------------+
|  Event Ring (mmap)   |  Hugepage-backed ring buffer
|  Zero-copy read      |  Nanosecond-precision timestamps
+--------+------------+
         | blocking thread
         v
+---------------------+
|  Event Listener      |  Reads ring, converts C structs -> Rust
+--------+------------+
         | mpsc channel (100K buffer)
         v
+---------------------+
|  Event Forwarder     |  Lifecycle tracker + TPS + contention + top-K
|  +- server_seqno     |  Assigns monotonic seqno to every item
|  +- ring buffer      |  Pre-serializes & stores for cursor resume
|  +- commit_stage     |  Attaches finality info to every event
+--------+------------+
         | broadcast channel (1M buffer)
         v
+---------------------+
|  axum Server :8443   |
|  +- /v1/ws/*         |  5 WebSocket channels + subscriptions
|  +- backpressure     |  Per-client bounded buffer (4096)
|  +- cursor resume    |  ?resume_from=<seqno> on reconnect
|  +- /v1/blocks/*     |  Lifecycle REST queries
|  +- /v1/tps          |  REST snapshots
|  +- /health          |  Health check
+---------------------+
```

## Computed Metrics

**Block Lifecycle** — Tracks each block through MonadBFT consensus stages: Proposed -> Voted (~400ms, speculative finality) -> Finalized (~800ms, full finality) -> Verified (state root confirmed). Every event carries its block's `commit_stage` so clients know finality confidence.

**TPS** — 2.5-block rolling window transaction count (~1 second at Monad's ~400ms block time).

**Contention Data** — Per-block analysis: parallel execution efficiency, contended storage slots, top contended contracts, contract co-access graph.

**Top Accesses** — Space-Saving algorithm tracking most frequently accessed accounts and storage slots (reset every 5 minutes).

## Documentation

- [API Reference](docs/API.md) — Endpoints, wire format, subscription protocol, cursor resume
- [Execution Events Reference](docs/EVENTS.md) — All 25 event types with field descriptions
- [Deployment Guide](docs/DEPLOYMENT.md) — Docker, native build, systemd, firewall

## Examples

| Example | Description |
|---------|-------------|
| [track-lifecycle.ts](examples/track-lifecycle.ts) | Watch blocks through Proposed/Voted/Finalized/Verified stages |
| [subscribe-blocks.ts](examples/subscribe-blocks.ts) | Track block events via `/v1/ws/blocks` channel |
| [monitor-contention.ts](examples/monitor-contention.ts) | Monitor parallel execution via `/v1/ws/contention` channel |
| [track-storage.ts](examples/track-storage.ts) | Watch storage access with client-driven subscriptions |
| [websocket-test.sh](examples/websocket-test.sh) | Quick CLI test with websocat |

## Project Structure

```
monad-execution-gateway/
+-- gateway/                 # Rust WebSocket + REST gateway server
|   +-- src/
|   |   +-- bin/gateway.rs   # Entry point
|   |   +-- lib/             # Event listener, server, filters, analytics
|   +-- Dockerfile
|   +-- build.sh             # Native build helper
|   +-- restricted_filters.json
+-- sdk/typescript/          # TypeScript client SDK
|   +-- src/
|       +-- client.ts        # GatewayClient class (auto cursor resume)
|       +-- types.ts         # Full type definitions
+-- docs/                    # API and deployment documentation
+-- examples/                # Usage examples
+-- docker-compose.yml
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

MIT — see [LICENSE](LICENSE).
