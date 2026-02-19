# Monad Execution Events Gateway

Community-maintained public Execution Events Gateway for Monad builders. Real-time execution event streaming from a Monad validator node with **EVM-internal visibility** not available through standard Ethereum JSON-RPC.

## What This Streams

| Data | Standard RPC | This Gateway |
|------|:---:|:---:|
| Block headers, logs, tx receipts | yes | yes |
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

# Blocks only
websocat ws://your-validator:8443/v1/ws/blocks

# REST snapshot
curl http://your-validator:8443/v1/tps
curl http://your-validator:8443/v1/status
```

```typescript
// TypeScript SDK
import { GatewayClient } from "@monad-labs/execution-events";

const client = new GatewayClient({
  url: "ws://your-validator:8443",
  channel: "blocks",   // "all" | "blocks" | "txs" | "contention"
});

client.on("event", (event) => {
  console.log(event.event_name, event.payload);
});

client.on("tps", (tps) => {
  console.log("TPS:", tps);
});

await client.connect();

// Refine with client-driven subscriptions
client.subscribe(["BlockFinalized", "TPS"]);
```

```typescript
// REST helpers (no WebSocket needed)
const { tps } = await GatewayClient.fetchTPS("http://your-validator:8443");
const status = await GatewayClient.fetchStatus("http://your-validator:8443");
```

## API Endpoints

All endpoints served on a single port (default `8443`):

| Endpoint | Type | Description |
|----------|------|-------------|
| `/v1/ws` | WebSocket | All events + all computed metrics |
| `/v1/ws/blocks` | WebSocket | Block lifecycle events + TPS |
| `/v1/ws/txs` | WebSocket | Transaction events only |
| `/v1/ws/contention` | WebSocket | Contention data only |
| `/v1/tps` | REST | Current TPS snapshot |
| `/v1/contention` | REST | Latest per-block contention data |
| `/v1/status` | REST | Gateway status |
| `/health` | REST | Health check |

## Architecture

```
Monad Validator Node
        │
        ▼
┌─────────────────────┐
│  Event Ring (mmap)   │  Hugepage-backed ring buffer
│  Zero-copy read      │  Nanosecond-precision timestamps
└────────┬────────────┘
         │ blocking thread
         ▼
┌─────────────────────┐
│  Event Listener      │  Reads ring, converts C structs → Rust
└────────┬────────────┘
         │ mpsc channel (100K buffer)
         ▼
┌─────────────────────┐
│  Event Forwarder     │  Computes: TPS, contention, top-K accesses
└────────┬────────────┘
         │ broadcast channel (1M buffer)
         ▼
┌─────────────────────┐
│  axum Server :8443   │
│  ├─ /v1/ws/*         │  Multi-channel WebSocket + subscriptions
│  ├─ /v1/tps          │  REST snapshots
│  ├─ /v1/contention   │
│  ├─ /v1/status       │
│  └─ /health          │  Health check (merged)
└─────────────────────┘
```

## Computed Metrics

The gateway computes three real-time analytics on the server side:

**TPS** — 2.5-block rolling window transaction count (~1 second at Monad's ~400ms block time)

**Contention Data** — Per-block analysis:
- Parallel execution efficiency (% of time saved by parallel execution)
- Contended storage slots (accessed by 2+ transactions)
- Top contended contracts ranked by contention score
- Contract co-access graph (which contracts share transactions)

**Top Accesses** — Space-Saving algorithm tracking most frequently accessed accounts and storage slots (reset every 5 minutes)

## Documentation

- [API Reference](docs/API.md) — Endpoints, subscription protocol, message formats
- [Execution Events Reference](docs/EVENTS.md) — All 25 event types with field descriptions
- [Deployment Guide](docs/DEPLOYMENT.md) — Docker, native build, systemd, firewall

## Examples

| Example | Description |
|---------|-------------|
| [subscribe-blocks.ts](examples/subscribe-blocks.ts) | Track block lifecycle via `/v1/ws/blocks` channel |
| [monitor-contention.ts](examples/monitor-contention.ts) | Monitor parallel execution via `/v1/ws/contention` channel |
| [track-storage.ts](examples/track-storage.ts) | Watch storage access with client-driven subscriptions |
| [websocket-test.sh](examples/websocket-test.sh) | Quick CLI test with websocat |

## Project Structure

```
monad-execution-gateway/
├── gateway/                 # Rust WebSocket + REST gateway server
│   ├── src/
│   │   ├── bin/gateway.rs   # Entry point
│   │   └── lib/             # Event listener, server, filters, analytics
│   ├── Dockerfile
│   ├── build.sh             # Native build helper
│   └── restricted_filters.json
├── sdk/typescript/          # TypeScript client SDK
│   └── src/
│       ├── client.ts        # GatewayClient class
│       └── types.ts         # Full type definitions
├── docs/                    # API and deployment documentation
├── examples/                # Usage examples
└── docker-compose.yml
```

## Who Uses This

- **Trading teams** — Real-time TPS and contention signals for market stress detection
- **Bot builders** — Storage access patterns for transaction ordering optimization
- **Game studios** — State partition analysis for parallel execution optimization
- **Analytics platforms** — Execution-level metrics unavailable from RPC
- **Validator operators** — Node health monitoring and performance metrics

## License

MIT
