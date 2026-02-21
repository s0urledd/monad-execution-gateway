"""Monad Execution Events — asyncio WebSocket Client.

Features:
- Auto-reconnect with exponential back-off and jitter
- Cursor resume (tracks server_seqno, reconnects with ?resume_from)
- Resume ACK validation
- Heartbeat detection — reconnects on silence
- Channel abstraction (lifecycle / raw / contention / blocks / txs)
- Typed events

Example::

    import asyncio
    from monad_execution_events import GatewayClient, GatewayClientOptions, Channel

    async def main():
        opts = GatewayClientOptions(
            url="wss://gateway.monad.xyz",
            channel=Channel.LIFECYCLE,
        )
        client = GatewayClient(opts)

        @client.on("lifecycle")
        def on_lifecycle(update):
            if update.to_stage.value == "Finalized":
                print(f"Block {update.block_number} finalized in {update.block_age_ms}ms")

        await client.connect()
        await client.listen_forever()

    asyncio.run(main())
"""

from __future__ import annotations

import asyncio
import json
import logging
import random
from collections import defaultdict
from typing import Any, Callable

import aiohttp
import websockets
import websockets.exceptions

from .types import (
    BlockLifecycleUpdate,
    BlockStage,
    Channel,
    CHANNEL_PATHS,
    ContentionData,
    EventName,
    ExecEvent,
    GatewayClientOptions,
    ResumeMode,
    ServerMessage,
)

logger = logging.getLogger("monad_execution_events")

EventCallback = Callable[..., Any]


class GatewayClient:
    """Async WebSocket client for the Monad Execution Events Gateway."""

    def __init__(self, options: GatewayClientOptions) -> None:
        self._opts = options
        self._ws: websockets.WebSocketClientProtocol | None = None
        self._last_seqno: int | None = None
        self._listeners: dict[str, list[EventCallback]] = defaultdict(list)
        self._should_reconnect = True
        self._reconnect_attempts = 0
        self._pending_subscription: dict[str, Any] | None = None
        self._heartbeat_task: asyncio.Task[None] | None = None
        self._connected = False

    # ── Event Registration ───────────────────────────────────────────

    def on(self, event: str) -> Callable[[EventCallback], EventCallback]:
        """Decorator to register an event listener.

        Usage::

            @client.on("lifecycle")
            def handle(update):
                print(update.block_number)
        """
        def decorator(fn: EventCallback) -> EventCallback:
            self._listeners[event].append(fn)
            return fn
        return decorator

    def add_listener(self, event: str, callback: EventCallback) -> None:
        self._listeners[event].append(callback)

    def remove_listener(self, event: str, callback: EventCallback) -> None:
        try:
            self._listeners[event].remove(callback)
        except ValueError:
            pass

    # ── Connection ───────────────────────────────────────────────────

    async def connect(self) -> None:
        """Open the WebSocket connection."""
        self._should_reconnect = True
        url = self._build_ws_url()
        logger.info("Connecting to %s", url)
        self._ws = await websockets.connect(url)
        self._connected = True
        self._reconnect_attempts = 0
        self._emit("connected")
        self._reset_heartbeat()

        # Re-send pending subscription
        if self._pending_subscription and self._ws:
            await self._ws.send(json.dumps(self._pending_subscription))

    async def disconnect(self) -> None:
        """Cleanly disconnect. Stops auto-reconnect."""
        self._should_reconnect = False
        self._cancel_heartbeat()
        if self._ws:
            await self._ws.close()
            self._ws = None
        self._connected = False

    async def listen_forever(self) -> None:
        """Block and process messages. Reconnects automatically on disconnect."""
        while self._should_reconnect:
            try:
                if not self._ws or self._ws.closed:
                    await self.connect()
                async for raw in self._ws:  # type: ignore[union-attr]
                    self._reset_heartbeat()
                    try:
                        data = json.loads(raw)
                        msg = ServerMessage.from_dict(data)
                        self._handle_message(msg)
                    except Exception:
                        logger.debug("Ignoring malformed message", exc_info=True)
            except (
                websockets.exceptions.ConnectionClosed,
                ConnectionError,
                OSError,
            ) as exc:
                self._connected = False
                self._cancel_heartbeat()
                self._emit("disconnected")
                logger.warning("Disconnected: %s", exc)
                if self._should_reconnect:
                    await self._backoff_reconnect()
            except asyncio.CancelledError:
                break

    # ── Subscription ─────────────────────────────────────────────────

    async def subscribe(
        self,
        events: list[str],
        *,
        filters: list[dict[str, Any]] | None = None,
        min_stage: BlockStage | None = None,
    ) -> None:
        """Send a subscription message.

        Simple::

            await client.subscribe(["BlockFinalized", "TPS"])

        Advanced::

            await client.subscribe(
                ["TxnLog"],
                filters=[{
                    "event_name": "TxnLog",
                    "field_filters": [
                        {"field": "address", "filter": {"values": ["0x..."]}}
                    ]
                }],
                min_stage=BlockStage.Finalized,
            )
        """
        if filters or min_stage:
            msg: dict[str, Any] = {"subscribe": {"events": events}}
            if filters:
                msg["subscribe"]["filters"] = filters
            if min_stage:
                msg["subscribe"]["min_stage"] = min_stage.value
        else:
            msg = {"subscribe": events}

        self._pending_subscription = msg

        if self._ws and not self._ws.closed:
            await self._ws.send(json.dumps(msg))

    # ── Resume ACK ───────────────────────────────────────────────────

    async def wait_for_resume(self, timeout: float = 5.0) -> ResumeMode:
        """Wait for the server's Resume ACK.

        Returns ``ResumeMode`` with ``mode="resume"`` (lossless) or
        ``mode="snapshot"`` (state snapshot after gap).
        """
        future: asyncio.Future[ResumeMode] = asyncio.get_event_loop().create_future()

        def _on_resume(info: ResumeMode) -> None:
            if not future.done():
                future.set_result(info)

        self._listeners["resume"].append(_on_resume)
        try:
            return await asyncio.wait_for(future, timeout)
        finally:
            self.remove_listener("resume", _on_resume)

    # ── Getters ──────────────────────────────────────────────────────

    @property
    def connected(self) -> bool:
        return self._connected and self._ws is not None and not self._ws.closed

    @property
    def server_seqno(self) -> int | None:
        return self._last_seqno

    # ── REST Helpers ─────────────────────────────────────────────────

    @staticmethod
    async def fetch_tps(base_url: str) -> dict[str, Any]:
        url = base_url.rstrip("/")
        async with aiohttp.ClientSession() as s:
            async with s.get(f"{url}/v1/tps") as r:
                return await r.json()  # type: ignore[return-value]

    @staticmethod
    async def fetch_status(base_url: str) -> dict[str, Any]:
        url = base_url.rstrip("/")
        async with aiohttp.ClientSession() as s:
            async with s.get(f"{url}/v1/status") as r:
                return await r.json()  # type: ignore[return-value]

    @staticmethod
    async def fetch_contention(base_url: str) -> dict[str, Any]:
        url = base_url.rstrip("/")
        async with aiohttp.ClientSession() as s:
            async with s.get(f"{url}/v1/contention") as r:
                return await r.json()  # type: ignore[return-value]

    @staticmethod
    async def fetch_lifecycle(base_url: str) -> list[dict[str, Any]]:
        url = base_url.rstrip("/")
        async with aiohttp.ClientSession() as s:
            async with s.get(f"{url}/v1/blocks/lifecycle") as r:
                return await r.json()  # type: ignore[return-value]

    # ── Private ──────────────────────────────────────────────────────

    def _build_ws_url(self) -> str:
        base = self._opts.url.rstrip("/")
        path = CHANNEL_PATHS[self._opts.channel]
        url = f"{base}{path}"
        if self._last_seqno is not None:
            sep = "&" if "?" in url else "?"
            url = f"{url}{sep}resume_from={self._last_seqno}"
        return url

    def _handle_message(self, msg: ServerMessage) -> None:
        self._last_seqno = msg.server_seqno

        if msg.resume:
            self._emit("resume", msg.resume)
        elif msg.events:
            self._emit("events", msg.events)
            for ev in msg.events:
                self._emit("event", ev)
        elif msg.tps is not None:
            self._emit("tps", msg.tps)
        elif msg.contention:
            self._emit("contention", msg.contention)
        elif msg.lifecycle:
            self._emit("lifecycle", msg.lifecycle)
        elif msg.top_accesses:
            self._emit("top_accesses", msg.top_accesses)

    def _emit(self, event: str, *args: Any) -> None:
        for handler in self._listeners.get(event, []):
            try:
                handler(*args)
            except Exception:
                logger.exception("Handler error for event '%s'", event)

    # ── Heartbeat ────────────────────────────────────────────────────

    def _reset_heartbeat(self) -> None:
        self._cancel_heartbeat()
        timeout = self._opts.heartbeat_timeout
        if timeout <= 0:
            return
        self._heartbeat_task = asyncio.get_event_loop().call_later(
            timeout, self._on_heartbeat_timeout
        )  # type: ignore[assignment]

    def _cancel_heartbeat(self) -> None:
        if self._heartbeat_task is not None:
            self._heartbeat_task.cancel()  # type: ignore[union-attr]
            self._heartbeat_task = None

    def _on_heartbeat_timeout(self) -> None:
        logger.warning("Heartbeat timeout — forcing reconnect")
        if self._ws:
            asyncio.ensure_future(self._ws.close())

    # ── Exponential Back-off ─────────────────────────────────────────

    async def _backoff_reconnect(self) -> None:
        self._reconnect_attempts += 1
        max_attempts = self._opts.max_reconnect_attempts
        if max_attempts is not None and self._reconnect_attempts > max_attempts:
            logger.error("Max reconnect attempts (%d) reached", max_attempts)
            self._should_reconnect = False
            return

        exp = min(
            self._opts.reconnect_delay * (2 ** (self._reconnect_attempts - 1)),
            self._opts.max_reconnect_delay,
        )
        jitter = random.random() * self._opts.reconnect_delay * 0.5
        delay = exp + jitter

        self._emit("reconnecting", self._reconnect_attempts, delay)
        logger.info(
            "Reconnecting in %.1fs (attempt %d)", delay, self._reconnect_attempts
        )
        await asyncio.sleep(delay)
