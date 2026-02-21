use lazy_static::lazy_static;
use prometheus::{
    opts, register_counter, register_gauge, register_histogram, Counter, Gauge, Histogram,
};

lazy_static! {
    /// Current number of active WebSocket connections.
    pub static ref WS_ACTIVE_CONNECTIONS: Gauge = register_gauge!(opts!(
        "ws_active_connections",
        "Number of active WebSocket connections"
    ))
    .unwrap();

    /// Total number of events broadcast to clients (across all connections).
    pub static ref WS_EVENTS_TOTAL: Counter = register_counter!(opts!(
        "ws_events_total",
        "Total events broadcast to clients"
    ))
    .unwrap();

    /// Total messages dropped due to per-client backpressure.
    pub static ref WS_DROPPED_TOTAL: Counter = register_counter!(opts!(
        "ws_dropped_total",
        "Total messages dropped due to slow-client backpressure"
    ))
    .unwrap();

    /// Total WebSocket disconnections.
    pub static ref WS_DISCONNECT_TOTAL: Counter = register_counter!(opts!(
        "ws_disconnect_total",
        "Total WebSocket disconnections"
    ))
    .unwrap();

    /// Total cursor-resume connections (client resumed from seqno delta).
    pub static ref RESUME_DELTA_TOTAL: Counter = register_counter!(opts!(
        "resume_delta_total",
        "Total cursor-resume connections (delta mode)"
    ))
    .unwrap();

    /// Total snapshot-replay connections (cursor miss or fresh connect).
    pub static ref RESUME_SNAPSHOT_TOTAL: Counter = register_counter!(opts!(
        "resume_snapshot_total",
        "Total snapshot-replay connections (cursor miss or fresh)"
    ))
    .unwrap();

    /// Block finalization latency in milliseconds (Proposed → Finalized).
    ///
    /// Grafana queries for percentiles:
    ///   p50:  histogram_quantile(0.50, rate(finalize_latency_ms_bucket[5m]))
    ///   p95:  histogram_quantile(0.95, rate(finalize_latency_ms_bucket[5m]))
    ///   p99:  histogram_quantile(0.99, rate(finalize_latency_ms_bucket[5m]))
    ///   avg:  rate(finalize_latency_ms_sum[5m]) / rate(finalize_latency_ms_count[5m])
    pub static ref FINALIZE_LATENCY_MS: Histogram = register_histogram!(
        "finalize_latency_ms",
        "Block finalization latency in ms (Proposed to Finalized)",
        // Dense buckets around expected Monad finality window for accurate p99
        vec![
            25.0, 50.0, 100.0, 150.0, 200.0, 300.0, 400.0, 500.0,
            600.0, 700.0, 800.0, 900.0, 1000.0, 1200.0, 1500.0,
            2000.0, 3000.0, 5000.0, 10000.0
        ]
    )
    .unwrap();

    /// Current number of entries in the broadcast ring buffer.
    pub static ref BROADCAST_QUEUE_USAGE: Gauge = register_gauge!(opts!(
        "broadcast_queue_usage",
        "Number of entries in the broadcast ring buffer"
    ))
    .unwrap();

    /// Ring buffer usage as percentage (0–100).
    /// broadcast_queue_usage_pct = len / capacity * 100
    pub static ref BROADCAST_QUEUE_USAGE_PCT: Gauge = register_gauge!(opts!(
        "broadcast_queue_usage_pct",
        "Broadcast ring buffer usage as percentage of capacity"
    ))
    .unwrap();

    /// Connections rejected due to per-IP limit.
    pub static ref WS_REJECTED_IP_LIMIT: Counter = register_counter!(opts!(
        "ws_rejected_ip_limit_total",
        "WebSocket connections rejected due to per-IP connection limit"
    ))
    .unwrap();

    /// Subscribe messages rejected due to per-client subscription limit.
    pub static ref WS_REJECTED_SUB_LIMIT: Counter = register_counter!(opts!(
        "ws_rejected_sub_limit_total",
        "Subscribe requests rejected due to per-client subscription limit"
    ))
    .unwrap();
}
