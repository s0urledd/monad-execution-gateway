use std::collections::{HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use alloy_primitives::{Address, B256};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use monad_exec_events::ExecEvent;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};

use prometheus::{Encoder, TextEncoder};

use crate::block_lifecycle::{BlockLifecycleTracker, BlockLifecycleUpdate, BlockStage};
use crate::contention_tracker::{ContentionData, ContentionTracker};
use crate::event_filter::{
    is_restricted_mode, load_restricted_filters, EventFilter, EventFilterSpec,
};
use crate::event_listener::{EventData, EventName};
use crate::metrics;
use crate::serializable_event::SerializableEventData;
use crate::top_k_tracker::{AccessEntry, TopKTracker};

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Shared Types
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopAccessesData {
    pub account: Vec<AccessEntry<Address>>,
    pub storage: Vec<AccessEntry<(Address, B256)>>,
}

#[derive(Debug, Clone)]
pub enum EventDataOrMetrics {
    Event(EventData),
    TopAccesses(TopAccessesData),
    TPS(usize),
    Contention(ContentionData),
    Lifecycle(BlockLifecycleUpdate),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Events(Vec<SerializableEventData>),
    TopAccesses(TopAccessesData),
    TPS(usize),
    ContentionData(ContentionData),
    Lifecycle(BlockLifecycleUpdate),
    /// Control message sent as the first frame after connect.
    /// `mode` is `"resume"` (cursor hit) or `"snapshot"` (full state replay).
    Resume(ResumeMode),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeMode {
    pub mode: String,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Cursor Resume & Backpressure Constants
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Maximum number of broadcast items kept in the resume ring buffer.
/// At ~1KB avg per item this is ~100MB worst case.
const MAX_EVENT_HISTORY: usize = 100_000;

/// Per-client outbound buffer capacity (in serialized JSON messages).
const CLIENT_SEND_BUFFER: usize = 4_096;

/// After this many dropped messages, disconnect the slow client.
const SLOW_CLIENT_DROP_LIMIT: u64 = 10_000;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Sequenced Broadcast Item
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Every broadcast item is tagged with a monotonic server-level seqno.
/// This seqno is independent of the per-event ring-buffer seqno.
#[derive(Debug, Clone)]
struct SequencedItem {
    seqno: u64,
    item: EventDataOrMetrics,
}

/// Ring buffer entry: pre-serialized JSON ready to replay byte-for-byte.
/// Avoids re-processing and re-serializing on cursor resume.
#[derive(Clone)]
struct ReplayEntry {
    seqno: u64,
    json: String,
}

/// Wire wrapper: every JSON message sent to clients carries a `server_seqno`.
/// The client stores the last seen value and passes it back as
/// `?resume_from=<server_seqno>` on reconnect.
#[derive(Serialize)]
struct WireMessage<'a> {
    server_seqno: u64,
    #[serde(flatten)]
    message: &'a ServerMessage,
}

/// Query parameters accepted on WebSocket upgrade endpoints.
#[derive(Deserialize, Default)]
struct WsQuery {
    /// If provided, the server replays all buffered items with
    /// `server_seqno > resume_from` before switching to live mode.
    #[serde(default)]
    resume_from: Option<u64>,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Gateway State
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub struct GatewayState {
    pub event_broadcast: broadcast::Sender<SequencedItem>,
    pub tps: watch::Receiver<usize>,
    pub contention: watch::Receiver<Option<ContentionData>>,
    pub block_number: AtomicU64,
    pub connected_clients: AtomicUsize,
    pub start_time: Instant,
    pub last_event_time: AtomicU64,
    pub base_filter: EventFilter,
    pub lifecycle_tracker: RwLock<BlockLifecycleTracker>,
    /// Monotonic counter for server-level message sequencing.
    pub server_seqno: AtomicU64,
    /// Ring buffer of pre-serialized wire messages for zero-cost cursor-resume.
    pub event_history: Mutex<VecDeque<ReplayEntry>>,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Channels & Subscriptions
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Debug, Clone)]
enum Channel {
    All,
    Blocks,
    Transactions,
    Contention,
    Lifecycle,
}

#[derive(Debug, Clone)]
struct ClientSubscription {
    event_names: HashSet<EventName>,
    include_tps: bool,
    include_contention: bool,
    include_top_accesses: bool,
    include_lifecycle: bool,
    /// Only deliver events from blocks that have reached this stage.
    /// `None` means deliver immediately (no stage gating).
    min_stage: Option<BlockStage>,
    field_filter: Option<EventFilter>,
}

impl ClientSubscription {
    fn all() -> Self {
        Self {
            event_names: HashSet::new(),
            include_tps: true,
            include_contention: true,
            include_top_accesses: true,
            include_lifecycle: true,
            min_stage: None,
            field_filter: None,
        }
    }

    fn from_channel(channel: &Channel) -> Self {
        match channel {
            Channel::All => Self::all(),
            Channel::Blocks => Self {
                event_names: [
                    EventName::BlockStart,
                    EventName::BlockEnd,
                    EventName::BlockQC,
                    EventName::BlockFinalized,
                    EventName::BlockVerified,
                    EventName::BlockReject,
                ]
                .into_iter()
                .collect(),
                include_tps: true,
                include_contention: false,
                include_top_accesses: false,
                include_lifecycle: true,
                min_stage: None,
                field_filter: None,
            },
            Channel::Transactions => Self {
                event_names: [
                    EventName::TxnHeaderStart,
                    EventName::TxnEvmOutput,
                    EventName::TxnLog,
                    EventName::TxnCallFrame,
                    EventName::TxnEnd,
                    EventName::TxnReject,
                    EventName::NativeTransfer,
                ]
                .into_iter()
                .collect(),
                include_tps: false,
                include_contention: false,
                include_top_accesses: false,
                include_lifecycle: false,
                min_stage: None,
                field_filter: None,
            },
            Channel::Contention => Self {
                event_names: HashSet::new(),
                include_tps: false,
                include_contention: true,
                include_top_accesses: false,
                include_lifecycle: false,
                min_stage: None,
                field_filter: None,
            },
            Channel::Lifecycle => Self {
                event_names: HashSet::new(),
                include_tps: false,
                include_contention: false,
                include_top_accesses: false,
                include_lifecycle: true,
                min_stage: None,
                field_filter: None,
            },
        }
    }

    fn wants_event(&self, event: &SerializableEventData, base_filter: &EventFilter) -> bool {
        if !base_filter.matches_event(event) {
            return false;
        }
        // Stage-aware filtering: skip events whose block hasn't reached min_stage
        if let Some(min_stage) = self.min_stage {
            match event.commit_stage {
                Some(stage) if stage >= min_stage => {}
                Some(_) => return false,
                // If commit_stage is unknown, let it through (block-level events
                // without a tracked block shouldn't be silently dropped)
                None => {}
            }
        }
        if self.event_names.is_empty() {
            return true;
        }
        if !self.event_names.contains(&event.event_name) {
            return false;
        }
        if let Some(ref filter) = self.field_filter {
            return filter.matches_event(event);
        }
        true
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Subscribe Protocol
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Deserialize)]
#[serde(untagged)]
enum SubscribeMessage {
    Simple { subscribe: Vec<String> },
    Advanced { subscribe: AdvancedSubscribeInner },
}

#[derive(Deserialize)]
struct AdvancedSubscribeInner {
    events: Vec<String>,
    #[serde(default)]
    filters: Vec<EventFilterSpec>,
    /// Only deliver events from blocks that have reached this commit stage.
    /// E.g. `"min_stage": "Finalized"` — only finalized block events.
    #[serde(default)]
    min_stage: Option<BlockStage>,
}

fn parse_subscribe(text: &str) -> Option<ClientSubscription> {
    let msg: SubscribeMessage = serde_json::from_str(text).ok()?;

    let (items, filters, min_stage) = match msg {
        SubscribeMessage::Simple { subscribe } => (subscribe, vec![], None),
        SubscribeMessage::Advanced { subscribe } => {
            (subscribe.events, subscribe.filters, subscribe.min_stage)
        }
    };

    let mut event_names = HashSet::new();
    let mut include_tps = false;
    let mut include_contention = false;
    let mut include_top_accesses = false;

    let mut include_lifecycle = false;

    for item in &items {
        match item.as_str() {
            "TPS" => include_tps = true,
            "ContentionData" => include_contention = true,
            "TopAccesses" => include_top_accesses = true,
            "Lifecycle" => include_lifecycle = true,
            name => {
                // Try PascalCase serde deserialization
                if let Ok(event_name) = serde_json::from_str::<EventName>(&format!("\"{}\"", name))
                {
                    event_names.insert(event_name);
                }
            }
        }
    }

    Some(ClientSubscription {
        event_names,
        include_tps,
        include_contention,
        include_top_accesses,
        include_lifecycle,
        min_stage,
        field_filter: if filters.is_empty() {
            None
        } else {
            Some(EventFilter::new(filters))
        },
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// TPS Tracker
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Default)]
struct TPSTracker {
    block_1_txs: usize,
    block_2_txs: usize,
    block_3_txs: usize,
    current_tx_count: usize,
}

impl TPSTracker {
    fn new() -> Self {
        Self::default()
    }

    fn record_tx(&mut self) {
        self.current_tx_count += 1;
    }

    fn get_tps(&mut self) -> usize {
        self.block_1_txs = self.block_2_txs;
        self.block_2_txs = self.block_3_txs;
        self.block_3_txs = self.current_tx_count;
        self.current_tx_count = 0;
        self.block_1_txs + self.block_2_txs + (self.block_3_txs / 2)
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// WebSocket Handlers
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn ws_all(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::All, q.resume_from))
}

async fn ws_blocks(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::Blocks, q.resume_from))
}

async fn ws_txs(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::Transactions, q.resume_from))
}

async fn ws_contention_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::Contention, q.resume_from))
}

async fn ws_lifecycle(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::Lifecycle, q.resume_from))
}

/// Build a snapshot of current gateway state to send on WebSocket connect.
/// This lets new clients immediately see in-progress blocks, current TPS, etc.
fn build_replay(channel: &Channel, state: &GatewayState) -> Vec<ServerMessage> {
    let mut msgs = Vec::new();

    match channel {
        Channel::All => {
            // TPS
            let tps = *state.tps.borrow();
            if tps > 0 {
                msgs.push(ServerMessage::TPS(tps));
            }
            // Latest contention
            if let Some(data) = state.contention.borrow().clone() {
                msgs.push(ServerMessage::ContentionData(data));
            }
            // Active lifecycle blocks
            if let Ok(lc) = state.lifecycle_tracker.read() {
                for update in lc.get_active_updates() {
                    msgs.push(ServerMessage::Lifecycle(update));
                }
            }
        }
        Channel::Blocks => {
            // TPS
            let tps = *state.tps.borrow();
            if tps > 0 {
                msgs.push(ServerMessage::TPS(tps));
            }
            // Active lifecycle blocks
            if let Ok(lc) = state.lifecycle_tracker.read() {
                for update in lc.get_active_updates() {
                    msgs.push(ServerMessage::Lifecycle(update));
                }
            }
        }
        Channel::Lifecycle => {
            // Active lifecycle blocks
            if let Ok(lc) = state.lifecycle_tracker.read() {
                for update in lc.get_active_updates() {
                    msgs.push(ServerMessage::Lifecycle(update));
                }
            }
        }
        Channel::Contention => {
            if let Some(data) = state.contention.borrow().clone() {
                msgs.push(ServerMessage::ContentionData(data));
            }
        }
        Channel::Transactions => {
            // No meaningful state to replay for raw transaction events
        }
    }

    msgs
}

async fn handle_ws(
    socket: WebSocket,
    state: Arc<GatewayState>,
    channel: Channel,
    resume_from: Option<u64>,
) {
    let client_id = state.connected_clients.fetch_add(1, Ordering::Relaxed);
    metrics::WS_ACTIVE_CONNECTIONS.inc();
    info!(
        "WebSocket connected: client-{} (channel: {:?}, resume_from: {:?})",
        client_id, channel, resume_from
    );

    let (ws_sender, mut receiver) = socket.split();
    let mut rx = state.event_broadcast.subscribe();
    let mut subscription = ClientSubscription::from_channel(&channel);

    // ── Backpressure: bounded channel between processor and WS sender ──
    let (client_tx, mut client_rx) = mpsc::channel::<String>(CLIENT_SEND_BUFFER);

    // Spawn a dedicated sender task that drains the bounded channel into the WS.
    let send_task = tokio::spawn(async move {
        let mut ws_sender = ws_sender;
        while let Some(json) = client_rx.recv().await {
            if ws_sender.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
        let _ = ws_sender.close().await;
    });

    // Helper: try_send with backpressure tracking
    let mut drop_count: u64 = 0;
    let mut total_sent: u64 = 0;
    let mut send_or_drop = |client_tx: &mpsc::Sender<String>,
                            json: String,
                            client_id: usize,
                            drop_count: &mut u64,
                            total_sent: &mut u64|
     -> bool {
        match client_tx.try_send(json) {
            Ok(_) => {
                *total_sent += 1;
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                *drop_count += 1;
                metrics::WS_DROPPED_TOTAL.inc();
                if *drop_count % 1000 == 0 {
                    warn!(
                        "client-{}: backpressure — dropped {} messages (sent {})",
                        client_id, drop_count, total_sent
                    );
                }
                if *drop_count >= SLOW_CLIENT_DROP_LIMIT {
                    warn!(
                        "client-{}: disconnecting slow consumer ({} drops)",
                        client_id, drop_count
                    );
                    return false; // signal disconnect
                }
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    };

    // ── Helper: send a control frame and bail on failure ──
    macro_rules! send_control {
        ($msg:expr) => {
            let wire = WireMessage {
                server_seqno: state.server_seqno.load(Ordering::Relaxed),
                message: &$msg,
            };
            if let Ok(json) = serde_json::to_string(&wire) {
                if !send_or_drop(
                    &client_tx,
                    json,
                    client_id,
                    &mut drop_count,
                    &mut total_sent,
                ) {
                    state.connected_clients.fetch_sub(1, Ordering::Relaxed);
                    metrics::WS_ACTIVE_CONNECTIONS.dec();
                    metrics::WS_DISCONNECT_TOTAL.inc();
                    send_task.abort();
                    return;
                }
            }
        };
    }

    // ── Helper: send snapshot replay (TPS / contention / lifecycle state) ──
    let send_snapshot = |client_tx: &mpsc::Sender<String>,
                         state: &Arc<GatewayState>,
                         channel: &Channel,
                         client_id: usize,
                         drop_count: &mut u64,
                         total_sent: &mut u64|
     -> bool {
        let replay_msgs = build_replay(channel, state);
        let current_seqno = state.server_seqno.load(Ordering::Relaxed);
        for msg in &replay_msgs {
            let wire = WireMessage {
                server_seqno: current_seqno,
                message: msg,
            };
            if let Ok(json) = serde_json::to_string(&wire) {
                if !send_or_drop(client_tx, json, client_id, drop_count, total_sent) {
                    return false;
                }
            }
        }
        if !replay_msgs.is_empty() {
            info!(
                "client-{} snapshot: sent {} state messages",
                client_id,
                replay_msgs.len()
            );
        }
        true
    };

    // ── Cursor resume OR snapshot replay ──
    let mut last_sent_seqno: u64 = 0;

    if let Some(cursor) = resume_from {
        // Try to replay pre-serialized entries from ring buffer
        let entries: Vec<ReplayEntry> = {
            let history = state.event_history.lock().unwrap();
            let oldest = history.front().map(|e| e.seqno).unwrap_or(0);
            if cursor < oldest && oldest > 0 {
                warn!(
                    "client-{}: resume_from={} too old (oldest={}), falling back to snapshot",
                    client_id, cursor, oldest
                );
                vec![]
            } else {
                history
                    .iter()
                    .filter(|e| e.seqno > cursor)
                    .cloned()
                    .collect()
            }
        };

        if entries.is_empty() {
            // Cursor too old or buffer empty → snapshot fallback
            metrics::RESUME_SNAPSHOT_TOTAL.inc();
            send_control!(ServerMessage::Resume(ResumeMode {
                mode: "snapshot".into()
            }));
            if !send_snapshot(
                &client_tx,
                &state,
                &channel,
                client_id,
                &mut drop_count,
                &mut total_sent,
            ) {
                state.connected_clients.fetch_sub(1, Ordering::Relaxed);
                metrics::WS_ACTIVE_CONNECTIONS.dec();
                metrics::WS_DISCONNECT_TOTAL.inc();
                send_task.abort();
                return;
            }
            last_sent_seqno = state.server_seqno.load(Ordering::Relaxed);
        } else {
            // Cursor hit → send Resume control, then replay stored JSON verbatim
            metrics::RESUME_DELTA_TOTAL.inc();
            send_control!(ServerMessage::Resume(ResumeMode {
                mode: "resume".into()
            }));
            let replay_count = entries.len();
            for entry in &entries {
                if !send_or_drop(
                    &client_tx,
                    entry.json.clone(),
                    client_id,
                    &mut drop_count,
                    &mut total_sent,
                ) {
                    state.connected_clients.fetch_sub(1, Ordering::Relaxed);
                    metrics::WS_ACTIVE_CONNECTIONS.dec();
                    metrics::WS_DISCONNECT_TOTAL.inc();
                    send_task.abort();
                    return;
                }
                last_sent_seqno = entry.seqno;
            }
            info!(
                "client-{} cursor resume: replayed {} entries from seqno {}",
                client_id, replay_count, cursor
            );
        }
    } else {
        // Fresh connect — snapshot replay
        metrics::RESUME_SNAPSHOT_TOTAL.inc();
        send_control!(ServerMessage::Resume(ResumeMode {
            mode: "snapshot".into()
        }));
        if !send_snapshot(
            &client_tx,
            &state,
            &channel,
            client_id,
            &mut drop_count,
            &mut total_sent,
        ) {
            state.connected_clients.fetch_sub(1, Ordering::Relaxed);
            metrics::WS_ACTIVE_CONNECTIONS.dec();
            metrics::WS_DISCONNECT_TOTAL.inc();
            send_task.abort();
            return;
        }
        last_sent_seqno = state.server_seqno.load(Ordering::Relaxed);
    }

    // ── Live event loop ──
    let mut events_buf: Vec<SerializableEventData> = Vec::new();
    let mut messages_buf: Vec<(u64, ServerMessage)> = Vec::new();

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(si) => {
                        // Skip items already sent during cursor replay
                        if si.seqno <= last_sent_seqno {
                            continue;
                        }
                        process_item_with_seqno(si.seqno, &si.item, &subscription, &state.base_filter, &state, &mut events_buf, &mut messages_buf);

                        // Drain available events without blocking
                        while let Ok(si) = rx.try_recv() {
                            if si.seqno <= last_sent_seqno {
                                continue;
                            }
                            process_item_with_seqno(si.seqno, &si.item, &subscription, &state.base_filter, &state, &mut events_buf, &mut messages_buf);
                        }

                        // Send batched events
                        if !events_buf.is_empty() {
                            // Use the seqno of the last item in messages_buf, or the si.seqno
                            let batch_seqno = messages_buf.last().map(|(s,_)| *s).unwrap_or(si.seqno);
                            let msg = ServerMessage::Events(std::mem::take(&mut events_buf));
                            let wire = WireMessage { server_seqno: batch_seqno, message: &msg };
                            if let Ok(json) = serde_json::to_string(&wire) {
                                if !send_or_drop(&client_tx, json, client_id, &mut drop_count, &mut total_sent) {
                                    break;
                                }
                            }
                        }

                        // Send metric messages
                        for (seqno, msg) in std::mem::take(&mut messages_buf) {
                            let wire = WireMessage { server_seqno: seqno, message: &msg };
                            if let Ok(json) = serde_json::to_string(&wire) {
                                if !send_or_drop(&client_tx, json, client_id, &mut drop_count, &mut total_sent) {
                                    break;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("client-{} lagged by {} events in broadcast", client_id, n);
                    }
                    Err(_) => break,
                }
            }

            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(new_sub) = parse_subscribe(&text) {
                            info!("client-{} updated subscription", client_id);
                            subscription = new_sub;
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }

    // Cleanup: drop the sender so the send_task exits
    drop(client_tx);
    let _ = send_task.await;
    state.connected_clients.fetch_sub(1, Ordering::Relaxed);
    metrics::WS_ACTIVE_CONNECTIONS.dec();
    metrics::WS_DISCONNECT_TOTAL.inc();
    if drop_count > 0 {
        info!(
            "WebSocket disconnected: client-{} (sent: {}, dropped: {})",
            client_id, total_sent, drop_count
        );
    } else {
        info!("WebSocket disconnected: client-{}", client_id);
    }
}

fn process_item_with_seqno(
    seqno: u64,
    item: &EventDataOrMetrics,
    subscription: &ClientSubscription,
    base_filter: &EventFilter,
    state: &GatewayState,
    events_buf: &mut Vec<SerializableEventData>,
    messages_buf: &mut Vec<(u64, ServerMessage)>,
) {
    match item {
        EventDataOrMetrics::Event(event_data) => {
            let mut serializable = SerializableEventData::from(event_data);
            // Attach commit_stage from lifecycle tracker
            if let Some(block_number) = event_data.block_number {
                if let Ok(lc) = state.lifecycle_tracker.read() {
                    serializable.commit_stage = lc.current_stage_by_number(block_number);
                }
            }
            if subscription.wants_event(&serializable, base_filter) {
                events_buf.push(serializable);
            }
        }
        EventDataOrMetrics::TPS(tps) => {
            if subscription.include_tps {
                messages_buf.push((seqno, ServerMessage::TPS(*tps)));
            }
        }
        EventDataOrMetrics::Contention(data) => {
            if subscription.include_contention {
                messages_buf.push((seqno, ServerMessage::ContentionData(data.clone())));
            }
        }
        EventDataOrMetrics::TopAccesses(data) => {
            if subscription.include_top_accesses {
                messages_buf.push((seqno, ServerMessage::TopAccesses(data.clone())));
            }
        }
        EventDataOrMetrics::Lifecycle(update) => {
            if subscription.include_lifecycle {
                messages_buf.push((seqno, ServerMessage::Lifecycle(update.clone())));
            }
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// REST Handlers
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn rest_tps(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    Json(serde_json::json!({ "tps": *state.tps.borrow() }))
}

async fn rest_contention(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    match state.contention.borrow().clone() {
        Some(data) => Json(serde_json::to_value(&data).unwrap()).into_response(),
        None => Json(serde_json::json!({ "error": "no contention data yet" })).into_response(),
    }
}

async fn rest_status(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = state.last_event_time.load(Ordering::Relaxed);
    let newest_seqno = state.server_seqno.load(Ordering::Relaxed);
    let oldest_seqno = state
        .event_history
        .lock()
        .unwrap()
        .front()
        .map(|e| e.seqno)
        .unwrap_or(0);

    Json(serde_json::json!({
        "status": if now.saturating_sub(last) <= 10 || last == 0 { "healthy" } else { "degraded" },
        "block_number": state.block_number.load(Ordering::Relaxed),
        "connected_clients": state.connected_clients.load(Ordering::Relaxed),
        "uptime_secs": state.start_time.elapsed().as_secs(),
        "last_event_age_secs": now.saturating_sub(last),
        "server_seqno": newest_seqno,
        "oldest_seqno": oldest_seqno,
        "newest_seqno": newest_seqno,
    }))
}

async fn rest_health(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = state.last_event_time.load(Ordering::Relaxed);
    let age = now.saturating_sub(last);

    if age >= 30 && last > 0 {
        error!("No events for {}s, exiting to trigger restart", age);
        std::process::exit(1);
    }

    Json(serde_json::json!({ "success": age <= 10 || last == 0 }))
}

async fn rest_lifecycle(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let lc = state.lifecycle_tracker.read().unwrap();
    let summaries = lc.get_all_lifecycles();
    Json(serde_json::to_value(&summaries).unwrap())
}

async fn rest_block_lifecycle(
    State(state): State<Arc<GatewayState>>,
    Path(block_number): Path<u64>,
) -> impl IntoResponse {
    let lc = state.lifecycle_tracker.read().unwrap();
    match lc.get_lifecycle_by_number(block_number) {
        Some(summary) => Json(serde_json::to_value(&summary).unwrap()).into_response(),
        None => Json(serde_json::json!({ "error": "block not found" })).into_response(),
    }
}

async fn rest_metrics() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    (
        [(axum::http::header::CONTENT_TYPE, encoder.format_type())],
        buffer,
    )
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Replay Serialization
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Pre-serialize a broadcast item into a wire-ready JSON string.
/// Called once in the forwarder; the resulting string is stored in the
/// ring buffer and replayed byte-for-byte on cursor resume.
fn serialize_for_replay(seqno: u64, item: &EventDataOrMetrics, state: &GatewayState) -> String {
    let msg = match item {
        EventDataOrMetrics::Event(event_data) => {
            let mut serializable = SerializableEventData::from(event_data);
            if let Some(block_number) = event_data.block_number {
                if let Ok(lc) = state.lifecycle_tracker.read() {
                    serializable.commit_stage = lc.current_stage_by_number(block_number);
                }
            }
            ServerMessage::Events(vec![serializable])
        }
        EventDataOrMetrics::TPS(tps) => ServerMessage::TPS(*tps),
        EventDataOrMetrics::Contention(data) => ServerMessage::ContentionData(data.clone()),
        EventDataOrMetrics::TopAccesses(data) => ServerMessage::TopAccesses(data.clone()),
        EventDataOrMetrics::Lifecycle(update) => ServerMessage::Lifecycle(update.clone()),
    };
    let wire = WireMessage {
        server_seqno: seqno,
        message: &msg,
    };
    serde_json::to_string(&wire).unwrap_or_default()
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Event Forwarder
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn run_event_forwarder(
    mut event_receiver: tokio::sync::mpsc::Receiver<EventData>,
    event_broadcast: broadcast::Sender<SequencedItem>,
    tps_tx: watch::Sender<usize>,
    contention_tx: watch::Sender<Option<ContentionData>>,
    state: Arc<GatewayState>,
) {
    let mut account_accesses = TopKTracker::new(1_000);
    let mut storage_accesses = TopKTracker::new(1_000);
    let mut accesses_reset_interval = tokio::time::interval(std::time::Duration::from_mins(5));

    let mut current_txn_hashes: Vec<Option<[u8; 32]>> = vec![None; 10_000];
    let mut tps_tracker = TPSTracker::new();
    let mut contention_tracker = ContentionTracker::new();
    let mut current_block_number: u64 = 0;

    loop {
        tokio::select! {
            event_data = event_receiver.recv() => {
                let Some(mut event_data) = event_data else {
                    error!("Event receiver closed");
                    return;
                };

                let now_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                state.last_event_time.store(now_secs, Ordering::Relaxed);

                if let EventName::TxnHeaderStart = event_data.event_name {
                    if let ExecEvent::TxnHeaderStart { txn_index, txn_header_start, .. } = &event_data.payload {
                        current_txn_hashes[*txn_index] = Some(txn_header_start.txn_hash.bytes);
                    }
                }

                if let Some(txn_idx) = event_data.txn_idx {
                    if let Some(Some(hash)) = current_txn_hashes.get(txn_idx) {
                        event_data.txn_hash = Some(*hash);
                    }
                }

                let mut tps_event = None;
                let mut contention_event = None;
                let mut lifecycle_event = None;

                // ── Lifecycle tracker: update BEFORE broadcasting ──
                // This ensures any consumer sees the correct commit stage.
                match event_data.event_name {
                    EventName::BlockStart => {
                        if let ExecEvent::BlockStart(block) = &event_data.payload {
                            let block_id = B256::from_slice(&block.block_tag.id.bytes);
                            let block_number = block.block_tag.block_number;
                            current_block_number = block_number;

                            let mut lc = state.lifecycle_tracker.write().unwrap();
                            let update = lc.on_block_proposed(block_id, block_number, event_data.timestamp_ns);
                            lifecycle_event = Some(EventDataOrMetrics::Lifecycle(update));

                            state.block_number.store(block_number, Ordering::Relaxed);
                            contention_tracker.on_block_start(block_number, event_data.timestamp_ns);

                            let tps = tps_tracker.get_tps();
                            let _ = tps_tx.send(tps);
                            tps_event = Some(EventDataOrMetrics::TPS(tps));
                        }
                    }
                    EventName::BlockEnd => {
                        if let ExecEvent::BlockEnd(end) = &event_data.payload {
                            let eth_hash = B256::from_slice(&end.eth_block_hash.bytes);
                            let gas_used = end.exec_output.gas_used;

                            // Internal signal: updates metadata, does NOT advance public stage
                            let mut lc = state.lifecycle_tracker.write().unwrap();
                            lc.on_execution_end(current_block_number, event_data.timestamp_ns, gas_used, eth_hash);
                        }

                        if let Some(data) = contention_tracker.on_block_end(event_data.timestamp_ns) {
                            let _ = contention_tx.send(Some(data.clone()));
                            contention_event = Some(EventDataOrMetrics::Contention(data));
                        }
                    }
                    EventName::BlockQC => {
                        // QC → Voted (public stage transition)
                        if let ExecEvent::BlockQC(qc) = &event_data.payload {
                            let block_id = B256::from_slice(&qc.block_tag.id.bytes);
                            let block_number = qc.block_tag.block_number;

                            let mut lc = state.lifecycle_tracker.write().unwrap();
                            if let Some(update) = lc.advance_stage(block_id, block_number, BlockStage::Voted, event_data.timestamp_ns) {
                                lifecycle_event = Some(EventDataOrMetrics::Lifecycle(update));
                            }
                        }
                    }
                    EventName::BlockFinalized => {
                        if let ExecEvent::BlockFinalized(fin) = &event_data.payload {
                            let block_id = B256::from_slice(&fin.id.bytes);
                            let block_number = fin.block_number;

                            let mut lc = state.lifecycle_tracker.write().unwrap();
                            if let Some(update) = lc.advance_stage(block_id, block_number, BlockStage::Finalized, event_data.timestamp_ns) {
                                metrics::FINALIZE_LATENCY_MS.observe(update.block_age_ms);
                                lifecycle_event = Some(EventDataOrMetrics::Lifecycle(update));
                            }
                        }
                    }
                    EventName::BlockVerified => {
                        if let ExecEvent::BlockVerified(ver) = &event_data.payload {
                            let block_number = ver.block_number;

                            let mut lc = state.lifecycle_tracker.write().unwrap();
                            // BlockVerified only has block_number — advance_stage falls back to number lookup
                            if let Some(update) = lc.advance_stage(B256::ZERO, block_number, BlockStage::Verified, event_data.timestamp_ns) {
                                lifecycle_event = Some(EventDataOrMetrics::Lifecycle(update));
                            }
                        }
                    }
                    EventName::BlockReject => {
                        // Rejected can happen from any stage
                        let mut lc = state.lifecycle_tracker.write().unwrap();
                        if let Some(update) = lc.advance_stage(B256::ZERO, current_block_number, BlockStage::Rejected, event_data.timestamp_ns) {
                            lifecycle_event = Some(EventDataOrMetrics::Lifecycle(update));
                        }
                    }
                    EventName::TxnHeaderStart => {
                        tps_tracker.record_tx();
                        if let Some(txn_idx) = event_data.txn_idx {
                            contention_tracker.on_txn_start(txn_idx, event_data.timestamp_ns);
                        }
                        // Track transaction count in lifecycle
                        {
                            let mut lc = state.lifecycle_tracker.write().unwrap();
                            lc.record_txn(current_block_number);
                        }
                    }
                    EventName::TxnEnd => {
                        if let Some(txn_idx) = event_data.txn_idx {
                            current_txn_hashes[txn_idx] = None;
                            contention_tracker.on_txn_end(txn_idx, event_data.timestamp_ns);
                        }
                    }
                    EventName::AccountAccess => {
                        if let ExecEvent::AccountAccess { account_access, .. } = &event_data.payload {
                            account_accesses.record(Address::from_slice(&account_access.address.bytes));
                        }
                    }
                    EventName::StorageAccess => {
                        if let ExecEvent::StorageAccess { storage_access, .. } = &event_data.payload {
                            let address = Address::from_slice(&storage_access.address.bytes);
                            let key = B256::from_slice(&storage_access.key.bytes);
                            storage_accesses.record((address, key));
                            contention_tracker.on_storage_access(address, key, event_data.txn_idx);
                        }
                    }
                    _ => (),
                }

                // ── Broadcast: event first, then metrics ──
                // Helper closure: assign seqno, pre-serialize for ring buffer, broadcast
                let mut broadcast_item = |item: EventDataOrMetrics, state: &Arc<GatewayState>, tx: &broadcast::Sender<SequencedItem>| {
                    let seqno = state.server_seqno.fetch_add(1, Ordering::Relaxed) + 1;
                    // Pre-serialize once for the ring buffer (zero-cost replay)
                    let json = serialize_for_replay(seqno, &item, state);
                    {
                        let mut history = state.event_history.lock().unwrap();
                        history.push_back(ReplayEntry { seqno, json });
                        while history.len() > MAX_EVENT_HISTORY {
                            history.pop_front();
                        }
                        metrics::BROADCAST_QUEUE_USAGE.set(history.len() as f64);
                    }
                    metrics::WS_EVENTS_TOTAL.inc();
                    // Broadcast raw item for live clients (per-subscription filtering)
                    let si = SequencedItem { seqno, item };
                    let _ = tx.send(si);
                };

                let send_accesses = event_data.event_name == EventName::BlockEnd;
                broadcast_item(EventDataOrMetrics::Event(event_data), &state, &event_broadcast);

                if send_accesses {
                    broadcast_item(EventDataOrMetrics::TopAccesses(TopAccessesData {
                        account: account_accesses.top_k(10),
                        storage: storage_accesses.top_k(10),
                    }), &state, &event_broadcast);
                }
                if let Some(e) = tps_event { broadcast_item(e, &state, &event_broadcast); }
                if let Some(e) = contention_event { broadcast_item(e, &state, &event_broadcast); }
                if let Some(e) = lifecycle_event { broadcast_item(e, &state, &event_broadcast); }
            }
            _ = accesses_reset_interval.tick() => {
                account_accesses.reset();
                storage_accesses.reset();
            }
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Server Entry Point
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub async fn run_server(
    addr: SocketAddr,
    event_receiver: tokio::sync::mpsc::Receiver<EventData>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (event_broadcast, _) = broadcast::channel::<SequencedItem>(1_000_000);
    let (tps_tx, tps_rx) = watch::channel(0usize);
    let (contention_tx, contention_rx) = watch::channel(None::<ContentionData>);

    let base_filter = if is_restricted_mode() {
        info!("Running in restricted mode");
        load_restricted_filters()
    } else {
        info!("Running in unrestricted mode (all events)");
        EventFilter::default()
    };

    let state = Arc::new(GatewayState {
        event_broadcast: event_broadcast.clone(),
        tps: tps_rx,
        contention: contention_rx,
        block_number: AtomicU64::new(0),
        connected_clients: AtomicUsize::new(0),
        start_time: Instant::now(),
        last_event_time: AtomicU64::new(0),
        base_filter,
        lifecycle_tracker: RwLock::new(BlockLifecycleTracker::new()),
        server_seqno: AtomicU64::new(0),
        event_history: Mutex::new(VecDeque::with_capacity(MAX_EVENT_HISTORY)),
    });

    tokio::spawn(run_event_forwarder(
        event_receiver,
        event_broadcast,
        tps_tx,
        contention_tx,
        state.clone(),
    ));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // WebSocket channels
        .route("/v1/ws", get(ws_all))
        .route("/v1/ws/blocks", get(ws_blocks))
        .route("/v1/ws/txs", get(ws_txs))
        .route("/v1/ws/contention", get(ws_contention_handler))
        .route("/v1/ws/lifecycle", get(ws_lifecycle))
        // REST
        .route("/v1/tps", get(rest_tps))
        .route("/v1/contention", get(rest_contention))
        .route("/v1/status", get(rest_status))
        .route("/v1/blocks/lifecycle", get(rest_lifecycle))
        .route(
            "/v1/blocks/:block_number/lifecycle",
            get(rest_block_lifecycle),
        )
        // Metrics
        .route("/metrics", get(rest_metrics))
        // Health
        .route("/health", get(rest_health))
        // Backward compat: root path = all events (same handler, supports ?resume_from)
        .route("/", get(ws_all))
        .layer(cors)
        .with_state(state);

    info!("Execution Events Gateway listening on {}", addr);
    info!("  WebSocket:  ws://{}/v1/ws", addr);
    info!("  Blocks:     ws://{}/v1/ws/blocks", addr);
    info!("  Txs:        ws://{}/v1/ws/txs", addr);
    info!("  Contention: ws://{}/v1/ws/contention", addr);
    info!("  Lifecycle:  ws://{}/v1/ws/lifecycle", addr);
    info!("  REST:       http://{}/v1/tps", addr);
    info!("              http://{}/v1/contention", addr);
    info!("              http://{}/v1/status", addr);
    info!("              http://{}/v1/blocks/lifecycle", addr);
    info!("  Metrics:    http://{}/metrics", addr);
    info!("  Health:     http://{}/health", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
