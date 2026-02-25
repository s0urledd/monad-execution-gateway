# monad-execution-events (Python)

Python SDK for the Monad Execution Events Gateway. Subscribe to real-time execution events from Monad full nodes over WebSocket.

## Installation

```bash
pip install monad-execution-events
```

## Quick Start

```python
import asyncio
from monad_execution_events import GatewayClient, Channel

async def main():
    client = GatewayClient("ws://localhost:8443")
    await client.connect()
    await client.subscribe(Channel.BLOCK_LIFECYCLE)

    async for event in client.events():
        print(event)

asyncio.run(main())
```

## License

MIT
