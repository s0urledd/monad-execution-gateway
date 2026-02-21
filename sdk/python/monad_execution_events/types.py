"""Monad Execution Events — Type Definitions.

These types map 1:1 to the gateway's JSON wire format.
All hex values (addresses, hashes, U256) are 0x-prefixed strings.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Literal


# ── Event Names ──────────────────────────────────────────────────────

class EventName(str, Enum):
    RecordError = "RecordError"
    BlockStart = "BlockStart"
    BlockReject = "BlockReject"
    BlockPerfEvmEnter = "BlockPerfEvmEnter"
    BlockPerfEvmExit = "BlockPerfEvmExit"
    BlockEnd = "BlockEnd"
    BlockQC = "BlockQC"
    BlockFinalized = "BlockFinalized"
    BlockVerified = "BlockVerified"
    TxnHeaderStart = "TxnHeaderStart"
    TxnAccessListEntry = "TxnAccessListEntry"
    TxnAuthListEntry = "TxnAuthListEntry"
    TxnHeaderEnd = "TxnHeaderEnd"
    TxnReject = "TxnReject"
    TxnPerfEvmEnter = "TxnPerfEvmEnter"
    TxnPerfEvmExit = "TxnPerfEvmExit"
    TxnEvmOutput = "TxnEvmOutput"
    TxnLog = "TxnLog"
    TxnCallFrame = "TxnCallFrame"
    TxnEnd = "TxnEnd"
    AccountAccessListHeader = "AccountAccessListHeader"
    AccountAccess = "AccountAccess"
    StorageAccess = "StorageAccess"
    EvmError = "EvmError"
    NativeTransfer = "NativeTransfer"


# ── Block Stages ─────────────────────────────────────────────────────

class BlockStage(str, Enum):
    """MonadBFT consensus stages.

    Happy path: Proposed → Voted → Finalized → Verified
    """
    Proposed = "Proposed"
    Voted = "Voted"
    Finalized = "Finalized"
    Verified = "Verified"
    Rejected = "Rejected"


# ── Channels ─────────────────────────────────────────────────────────

class Channel(str, Enum):
    ALL = "all"
    BLOCKS = "blocks"
    TXS = "txs"
    CONTENTION = "contention"
    LIFECYCLE = "lifecycle"


CHANNEL_PATHS: dict[Channel, str] = {
    Channel.ALL: "/v1/ws",
    Channel.BLOCKS: "/v1/ws/blocks",
    Channel.TXS: "/v1/ws/txs",
    Channel.CONTENTION: "/v1/ws/contention",
    Channel.LIFECYCLE: "/v1/ws/lifecycle",
}


# ── Wire Types ───────────────────────────────────────────────────────

@dataclass
class ExecEvent:
    """Single execution event from the gateway."""
    event_name: EventName
    payload: dict[str, Any]
    seqno: int
    timestamp_ns: int
    block_number: int | None = None
    txn_idx: int | None = None
    txn_hash: str | None = None
    commit_stage: BlockStage | None = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> ExecEvent:
        stage = d.get("commit_stage")
        return cls(
            event_name=EventName(d["event_name"]),
            payload=d.get("payload", {}),
            seqno=d["seqno"],
            timestamp_ns=d["timestamp_ns"],
            block_number=d.get("block_number"),
            txn_idx=d.get("txn_idx"),
            txn_hash=d.get("txn_hash"),
            commit_stage=BlockStage(stage) if stage else None,
        )


@dataclass
class BlockLifecycleUpdate:
    block_hash: str
    block_number: int
    to_stage: BlockStage
    block_age_ms: float
    txn_count: int
    from_stage: BlockStage | None = None
    time_in_previous_stage_ms: float | None = None
    gas_used: int | None = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> BlockLifecycleUpdate:
        return cls(
            block_hash=d["block_hash"],
            block_number=d["block_number"],
            to_stage=BlockStage(d["to_stage"]),
            block_age_ms=d["block_age_ms"],
            txn_count=d["txn_count"],
            from_stage=BlockStage(d["from_stage"]) if d.get("from_stage") else None,
            time_in_previous_stage_ms=d.get("time_in_previous_stage_ms"),
            gas_used=d.get("gas_used"),
        )


@dataclass
class ContentionData:
    block_number: int
    block_wall_time_ns: int
    total_tx_time_ns: int
    parallel_efficiency_pct: float
    total_unique_slots: int
    contended_slot_count: int
    contention_ratio: float
    total_txn_count: int
    top_contended_slots: list[dict[str, Any]]
    top_contended_contracts: list[dict[str, Any]]
    contract_edges: list[dict[str, Any]]

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> ContentionData:
        return cls(**d)


@dataclass
class ResumeMode:
    mode: Literal["resume", "snapshot"]

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> ResumeMode:
        return cls(mode=d["mode"])


@dataclass
class ServerMessage:
    """Parsed wire message from the gateway."""
    server_seqno: int
    events: list[ExecEvent] | None = None
    tps: int | None = None
    contention: ContentionData | None = None
    lifecycle: BlockLifecycleUpdate | None = None
    resume: ResumeMode | None = None
    top_accesses: dict[str, Any] | None = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> ServerMessage:
        msg = cls(server_seqno=d.get("server_seqno", 0))
        if "Events" in d:
            msg.events = [ExecEvent.from_dict(e) for e in d["Events"]]
        elif "TPS" in d:
            msg.tps = d["TPS"]
        elif "ContentionData" in d:
            msg.contention = ContentionData.from_dict(d["ContentionData"])
        elif "Lifecycle" in d:
            msg.lifecycle = BlockLifecycleUpdate.from_dict(d["Lifecycle"])
        elif "Resume" in d:
            msg.resume = ResumeMode.from_dict(d["Resume"])
        elif "TopAccesses" in d:
            msg.top_accesses = d["TopAccesses"]
        return msg


# ── Client Options ───────────────────────────────────────────────────

@dataclass
class GatewayClientOptions:
    """Options for GatewayClient."""
    url: str
    channel: Channel = Channel.ALL
    auto_reconnect: bool = True
    reconnect_delay: float = 1.0
    max_reconnect_delay: float = 30.0
    max_reconnect_attempts: int | None = None  # None = unlimited
    heartbeat_timeout: float = 10.0  # 0 = disabled
