# Monad Execution Events Gateway

![Rust](https://img.shields.io/badge/Rust-000?logo=rust&logoColor=white)
![TypeScript](https://img.shields.io/badge/TypeScript-3178C6?logo=typescript&logoColor=white)
![Python](https://img.shields.io/badge/Python-3776AB?logo=python&logoColor=white)
![Docker](https://img.shields.io/badge/Docker-2496ED?logo=docker&logoColor=white)
![WebSocket](https://img.shields.io/badge/WebSocket-010101?logo=socketdotio&logoColor=white)
![Prometheus](https://img.shields.io/badge/Prometheus-E6522C?logo=prometheus&logoColor=white)

Real-time execution event streaming from a Monad full node. EVM-internal visibility not available through standard JSON-RPC.

## Prerequisites

- A running **Monad full node** with the Execution Events SDK enabled
- The node's event ring mounted at `/var/lib/hugetlbfs/user/monad/pagesize-2MB/event-rings/`
- The gateway must run on the **same machine** as the node (shared memory access)

## Quick Start

```bash
git clone https://github.com/s0urledd/monad-execution-gateway.git
cd monad-execution-gateway

# Docker (recommended)
docker compose up -d

# With monitoring (Prometheus + Grafana)
docker compose --profile monitoring up -d

# Native
cd gateway && ./build.sh --run
```

Connect:

```bash
websocat ws://localhost:8443/v1/ws/lifecycle
curl http://localhost:8443/v1/status
```

## Docs

- [Architecture](docs/ARCHITECTURE.md)
- [API Reference](docs/API.md)
- [Events Reference](docs/EVENTS.md)
- [Wire Protocol](docs/wire.md)
- [Behavioral Spec](docs/spec.md)
- [Deployment](docs/DEPLOYMENT.md)
- [SLO Definitions](docs/slo.md)

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
| `/metrics` | Prometheus metrics |

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
            <- {"server_seqno":0, "Hello":{"wire_version":1, "features":[...], ...}}
            <- {"server_seqno":0, "Resume":{"mode":"snapshot"}}
            <- {"server_seqno":5, "Events":[...]}
            ...disconnect at seqno 42...

Reconnect:  ws://host:8443/v1/ws?resume_from=42
            <- {"server_seqno":0, "Hello":{"wire_version":1, "features":[...], ...}}
            <- {"server_seqno":0, "Resume":{"mode":"resume"}}
            <- {"server_seqno":43, ...}  // picks up where you left off
```

## License

MIT
