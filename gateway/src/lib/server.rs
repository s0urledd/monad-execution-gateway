use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use alloy_primitives::{Address, B256};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use monad_exec_events::ExecEvent;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};

use crate::contention_tracker::{ContentionData, ContentionTracker};
use crate::event_filter::{is_restricted_mode, load_restricted_filters, EventFilter, EventFilterSpec};
use crate::event_listener::{EventData, EventName};
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Events(Vec<SerializableEventData>),
    TopAccesses(TopAccessesData),
    TPS(usize),
    ContentionData(ContentionData),
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Gateway State
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub struct GatewayState {
    pub event_broadcast: broadcast::Sender<EventDataOrMetrics>,
    pub tps: watch::Receiver<usize>,
    pub contention: watch::Receiver<Option<ContentionData>>,
    pub block_number: AtomicU64,
    pub connected_clients: AtomicUsize,
    pub start_time: Instant,
    pub last_event_time: AtomicU64,
    pub base_filter: EventFilter,
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
}

#[derive(Debug, Clone)]
struct ClientSubscription {
    event_names: HashSet<EventName>,
    include_tps: bool,
    include_contention: bool,
    include_top_accesses: bool,
    field_filter: Option<EventFilter>,
}

impl ClientSubscription {
    fn all() -> Self {
        Self {
            event_names: HashSet::new(),
            include_tps: true,
            include_contention: true,
            include_top_accesses: true,
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
                field_filter: None,
            },
            Channel::Contention => Self {
                event_names: HashSet::new(),
                include_tps: false,
                include_contention: true,
                include_top_accesses: false,
                field_filter: None,
            },
        }
    }

    fn wants_event(&self, event: &SerializableEventData, base_filter: &EventFilter) -> bool {
        if !base_filter.matches_event(event) {
            return false;
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
}

fn parse_subscribe(text: &str) -> Option<ClientSubscription> {
    let msg: SubscribeMessage = serde_json::from_str(text).ok()?;

    let (items, filters) = match msg {
        SubscribeMessage::Simple { subscribe } => (subscribe, vec![]),
        SubscribeMessage::Advanced { subscribe } => (subscribe.events, subscribe.filters),
    };

    let mut event_names = HashSet::new();
    let mut include_tps = false;
    let mut include_contention = false;
    let mut include_top_accesses = false;

    for item in &items {
        match item.as_str() {
            "TPS" => include_tps = true,
            "ContentionData" => include_contention = true,
            "TopAccesses" => include_top_accesses = true,
            name => {
                // Try PascalCase serde deserialization
                if let Ok(event_name) = serde_json::from_str::<EventName>(&format!("\"{}\"", name)) {
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

async fn ws_all(ws: WebSocketUpgrade, State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::All))
}

async fn ws_blocks(ws: WebSocketUpgrade, State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::Blocks))
}

async fn ws_txs(ws: WebSocketUpgrade, State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::Transactions))
}

async fn ws_contention_handler(ws: WebSocketUpgrade, State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, Channel::Contention))
}

async fn handle_ws(socket: WebSocket, state: Arc<GatewayState>, channel: Channel) {
    let client_id = state.connected_clients.fetch_add(1, Ordering::Relaxed);
    info!("WebSocket connected: client-{} (channel: {:?})", client_id, channel);

    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.event_broadcast.subscribe();
    let mut subscription = ClientSubscription::from_channel(&channel);

    let mut events_buf: Vec<SerializableEventData> = Vec::new();
    let mut messages_buf: Vec<ServerMessage> = Vec::new();

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(item) => {
                        process_item(&item, &subscription, &state.base_filter, &mut events_buf, &mut messages_buf);

                        // Drain available events without blocking
                        while let Ok(item) = rx.try_recv() {
                            process_item(&item, &subscription, &state.base_filter, &mut events_buf, &mut messages_buf);
                        }

                        // Send batched events
                        if !events_buf.is_empty() {
                            let msg = ServerMessage::Events(std::mem::take(&mut events_buf));
                            if let Ok(json) = serde_json::to_string(&msg) {
                                if sender.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }

                        // Send metric messages
                        for msg in std::mem::take(&mut messages_buf) {
                            if let Ok(json) = serde_json::to_string(&msg) {
                                if sender.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("client-{} lagged by {} events", client_id, n);
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

    state.connected_clients.fetch_sub(1, Ordering::Relaxed);
    info!("WebSocket disconnected: client-{}", client_id);
}

fn process_item(
    item: &EventDataOrMetrics,
    subscription: &ClientSubscription,
    base_filter: &EventFilter,
    events_buf: &mut Vec<SerializableEventData>,
    messages_buf: &mut Vec<ServerMessage>,
) {
    match item {
        EventDataOrMetrics::Event(event_data) => {
            let serializable = SerializableEventData::from(event_data);
            if subscription.wants_event(&serializable, base_filter) {
                events_buf.push(serializable);
            }
        }
        EventDataOrMetrics::TPS(tps) => {
            if subscription.include_tps {
                messages_buf.push(ServerMessage::TPS(*tps));
            }
        }
        EventDataOrMetrics::Contention(data) => {
            if subscription.include_contention {
                messages_buf.push(ServerMessage::ContentionData(data.clone()));
            }
        }
        EventDataOrMetrics::TopAccesses(data) => {
            if subscription.include_top_accesses {
                messages_buf.push(ServerMessage::TopAccesses(data.clone()));
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
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let last = state.last_event_time.load(Ordering::Relaxed);

    Json(serde_json::json!({
        "status": if now.saturating_sub(last) <= 10 || last == 0 { "healthy" } else { "degraded" },
        "block_number": state.block_number.load(Ordering::Relaxed),
        "connected_clients": state.connected_clients.load(Ordering::Relaxed),
        "uptime_secs": state.start_time.elapsed().as_secs(),
        "last_event_age_secs": now.saturating_sub(last),
    }))
}

async fn rest_health(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let last = state.last_event_time.load(Ordering::Relaxed);
    let age = now.saturating_sub(last);

    if age >= 30 && last > 0 {
        error!("No events for {}s, exiting to trigger restart", age);
        std::process::exit(1);
    }

    Json(serde_json::json!({ "success": age <= 10 || last == 0 }))
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Event Forwarder
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn run_event_forwarder(
    mut event_receiver: tokio::sync::mpsc::Receiver<EventData>,
    event_broadcast: broadcast::Sender<EventDataOrMetrics>,
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

                match event_data.event_name {
                    EventName::BlockStart => {
                        let tps = tps_tracker.get_tps();
                        let _ = tps_tx.send(tps);
                        tps_event = Some(EventDataOrMetrics::TPS(tps));
                        if let ExecEvent::BlockStart(block) = &event_data.payload {
                            state.block_number.store(block.block_tag.block_number, Ordering::Relaxed);
                            contention_tracker.on_block_start(block.block_tag.block_number, event_data.timestamp_ns);
                        }
                    }
                    EventName::TxnHeaderStart => {
                        tps_tracker.record_tx();
                        if let Some(txn_idx) = event_data.txn_idx {
                            contention_tracker.on_txn_start(txn_idx, event_data.timestamp_ns);
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

                let send_accesses = if let EventName::BlockEnd = event_data.event_name {
                    if let Some(data) = contention_tracker.on_block_end(event_data.timestamp_ns) {
                        let _ = contention_tx.send(Some(data.clone()));
                        contention_event = Some(EventDataOrMetrics::Contention(data));
                    }
                    true
                } else {
                    false
                };

                let _ = event_broadcast.send(EventDataOrMetrics::Event(event_data));

                if send_accesses {
                    let _ = event_broadcast.send(EventDataOrMetrics::TopAccesses(TopAccessesData {
                        account: account_accesses.top_k(10),
                        storage: storage_accesses.top_k(10),
                    }));
                }
                if let Some(e) = tps_event { let _ = event_broadcast.send(e); }
                if let Some(e) = contention_event { let _ = event_broadcast.send(e); }
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
    let (event_broadcast, _) = broadcast::channel::<EventDataOrMetrics>(1_000_000);
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
        // REST
        .route("/v1/tps", get(rest_tps))
        .route("/v1/contention", get(rest_contention))
        .route("/v1/status", get(rest_status))
        // Health
        .route("/health", get(rest_health))
        // Backward compat: root path = all events
        .route("/", get(ws_all))
        .layer(cors)
        .with_state(state);

    info!("Execution Events Gateway listening on {}", addr);
    info!("  WebSocket:  ws://{}/v1/ws", addr);
    info!("  Blocks:     ws://{}/v1/ws/blocks", addr);
    info!("  Txs:        ws://{}/v1/ws/txs", addr);
    info!("  Contention: ws://{}/v1/ws/contention", addr);
    info!("  REST:       http://{}/v1/tps", addr);
    info!("              http://{}/v1/contention", addr);
    info!("              http://{}/v1/status", addr);
    info!("  Health:     http://{}/health", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
