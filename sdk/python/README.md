# monad-execution-events (Python)

Python SDK for the Monad Execution Events Gateway. Subscribe to real-time EVM execution events from a Monad full node over WebSocket.

## Installation

```bash
pip install monad-execution-events
```

## Quick Start

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
        print(f"Block {update.block_number}: {update.to_stage.value} ({update.block_age_ms}ms)")

    await client.connect()
    await client.listen_forever()

asyncio.run(main())
```

## Channels

| Channel | Endpoint | Data |
|---------|----------|------|
| `Channel.ALL` | `/v1/ws` | All events + metrics |
| `Channel.BLOCKS` | `/v1/ws/blocks` | Block events + TPS |
| `Channel.TXS` | `/v1/ws/txs` | Transaction events |
| `Channel.CONTENTION` | `/v1/ws/contention` | Contention analytics |
| `Channel.LIFECYCLE` | `/v1/ws/lifecycle` | Block stage transitions |

## Features

- Auto-reconnect with exponential backoff and jitter
- Cursor resume (tracks `server_seqno`, reconnects with `?resume_from`)
- Resume ACK validation via `await client.wait_for_resume(timeout)`
- Heartbeat detection — reconnects on silence
- REST helpers: `fetch_tps`, `fetch_status`, `fetch_contention`, `fetch_lifecycle`, `fetch_block_lifecycle`

## License

MIT
