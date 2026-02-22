# Service Level Objectives

> Measurable targets for the Monad Execution Events Gateway.
> Each SLO has a defined metric, target, and measurement method.
>
> **Reference hardware**: All targets measured on a single-socket AMD EPYC 9004
> (or equivalent) with 32 GB RAM, NVMe storage, and the gateway co-located
> on the same machine as the Monad validator. Network-attached deployments
> or lower-spec hardware may see different numbers. Treat these as
> **observed baselines**, not hard guarantees.

---

## 1. Latency SLOs

### 1.1 Event Publish Latency

**Definition**: Time from event ring read to broadcast channel send (single event processing in the forwarder loop).

| Percentile | Observed | Measurement |
|------------|----------|-------------|
| p50 | < 500 us | `event_publish_latency_ns` histogram |
| p99 | < 5 ms | `event_publish_latency_ns` histogram |
| p99.9 | < 20 ms | `event_publish_latency_ns` histogram |

> These numbers assume the gateway is co-located with the validator and
> shares the event ring via mmap'd hugepages (zero-copy read). If the
> gateway reads from a network-attached ring or a replay log, expect
> higher latencies.

**Prometheus query:**
```promql
histogram_quantile(0.99, rate(event_publish_latency_ns_bucket[5m]))
```

### 1.2 Block Finalization Latency

**Definition**: Time from `BlockStart` (Proposed) to `BlockFinalized` (Finalized) as observed by the gateway.

| Percentile | Observed | Measurement |
|------------|----------|-------------|
| p50 | < 600 ms | `finalize_latency_ms` histogram |
| p95 | < 1000 ms | `finalize_latency_ms` histogram |
| p99 | < 2000 ms | `finalize_latency_ms` histogram |

> This is a property of the Monad network, not the gateway. The gateway
> observes and reports it. Values will change as the network evolves.

**Prometheus query:**
```promql
histogram_quantile(0.99, rate(finalize_latency_ms_bucket[5m]))
```

### 1.3 WebSocket Send Latency

**Definition**: Time to write a single message to the WebSocket (per-client, measured in the sender task).

| Percentile | Observed | Measurement |
|------------|----------|-------------|
| p50 | < 100 us | `ws_send_latency_ns` histogram |
| p99 | < 2 ms | `ws_send_latency_ns` histogram |

> Measured with clients on localhost. Remote clients over WAN will see
> higher write latencies dominated by TCP buffer pressure and RTT.

**Prometheus query:**
```promql
histogram_quantile(0.99, rate(ws_send_latency_ns_bucket[5m]))
```

### 1.4 Resume Replay Latency

**Definition**: Time to replay buffered messages on reconnect (measured for full 100K buffer replay).

| Observed | Measurement |
|----------|-------------|
| < 500 ms for full buffer | Manual measurement / logs |

---

## 2. Resource SLOs

### 2.1 Memory (RSS)

| Condition | Observed | Measurement |
|-----------|----------|-------------|
| 0 clients | < 200 MB | `process_resident_memory_bytes` gauge |
| 100 clients (firehose) | < 1 GB | `process_resident_memory_bytes` gauge |
| 500 clients (mixed) | < 2 GB | `process_resident_memory_bytes` gauge |

**Memory budget breakdown (0 clients):**
- Ring buffer (100K entries * ~500B avg): ~50 MB
- Lifecycle tracker (128 completed + active): ~1 MB
- Contention tracker state: ~5 MB
- TopK trackers (2 * 1000 entries): ~1 MB
- Broadcast channel (1M capacity): ~50 MB
- Base process + runtime: ~50 MB

**Per-client overhead:**
- Bounded send channel (4096 * ~500B avg): ~2 MB per client
- Subscription state: ~1 KB per client
- WebSocket buffers: ~64 KB per client

### 2.2 CPU

| Condition | Observed | Measurement |
|-----------|----------|-------------|
| Idle (0 events) | < 1% | `process_cpu_seconds_total` rate |
| Normal load (5K events/s, 10 clients) | < 25% single core | `process_cpu_seconds_total` rate |
| High load (50K events/s, 100 clients) | < 200% (2 cores) | `process_cpu_seconds_total` rate |

> The 50K events/s scenario is a synthetic stress test. Current Monad
> mainnet/testnet event rates are significantly lower (typically 1K–10K
> events/s depending on block size and transaction complexity).

### 2.3 File Descriptors

| Observed | Measurement |
|----------|-------------|
| < 1000 (under 500 clients) | `process_open_fds` gauge |

---

## 3. Availability SLOs

### 3.1 Gateway Uptime

| Target | Measurement |
|--------|-------------|
| 99.9% (8.7h downtime/year) | External monitoring via `/health` endpoint |

Recovery mechanism: Health check process exit + container restart.

### 3.2 Event Freshness

| Condition | Target |
|-----------|--------|
| Last event age | < 10s under normal operation |
| Health check exit | Triggered at 30s without events |

**Prometheus query:**
```promql
time() - gateway_last_event_timestamp_seconds
```

---

## 4. Throughput SLOs

### 4.1 Event Processing

| Metric | Observed | Notes |
|--------|----------|-------|
| Sustained event throughput | > 50,000 events/s | Synthetic benchmark, co-located validator |
| Event processing without drops | > 10,000 events/s with 50 clients | Realistic mixed workload |

> Current Monad block production does not sustain 50K events/s in normal
> operation. The 50K figure represents the gateway's processing headroom,
> not the expected network load. It was measured by feeding synthetic
> events through the ring buffer at maximum rate.

### 4.2 Client Capacity

| Metric | Observed |
|--------|----------|
| Max concurrent WebSocket connections | > 500 |
| Broadcast fan-out | O(1) per client (non-blocking) |

---

## 5. Data Integrity SLOs

These are **invariants**, not best-effort targets. Violation is a bug.

### 5.1 Event Ordering

| Property | Guarantee |
|----------|-----------|
| `server_seqno` monotonicity | 100% (invariant) |
| Per-block event ordering | 100% (inherited from event ring) |
| Lifecycle stage monotonicity | 100% (invariant, enforced by state machine) |

### 5.2 Resume Correctness

| Property | Guarantee |
|----------|-----------|
| Replayed messages byte-identical | 100% (invariant) |
| No duplicate seqnos on resume | 100% (dedup by last_sent_seqno) |
| Resume within buffer window | 100% lossless |

---

## 6. Metrics Reference

All metrics exposed at `/metrics` in Prometheus text format.

### 6.1 Gauges

| Metric | Description | SLO tie-in |
|--------|-------------|------------|
| `ws_active_connections` | Current WebSocket connections | Section 4.2 |
| `broadcast_queue_usage` | Ring buffer entry count | Section 2.1 |
| `broadcast_queue_usage_pct` | Ring buffer usage percentage | Section 2.1 |

### 6.2 Counters

| Metric | Description | SLO tie-in |
|--------|-------------|------------|
| `ws_events_total` | Total events broadcast | Section 4.1 |
| `ws_dropped_total` | Messages dropped (backpressure) | Section 4.1 |
| `ws_disconnect_total` | Total disconnections | Section 3.1 |
| `resume_delta_total` | Successful cursor resumes | Section 5.2 |
| `resume_snapshot_total` | Snapshot fallback resumes | Section 5.2 |
| `ws_heartbeat_timeout_total` | Clients disconnected by heartbeat timeout | Section 3.1 |

### 6.3 Histograms

| Metric | Buckets | SLO tie-in |
|--------|---------|------------|
| `finalize_latency_ms` | 25, 50, 100, 150, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 1200, 1500, 2000, 3000, 5000, 10000 | Section 1.2 |
| `event_publish_latency_ns` | 100, 500, 1000, 5000, 10000, 50000, 100000, 500000, 1000000, 5000000, 20000000 | Section 1.1 |
| `ws_send_latency_ns` | 100, 500, 1000, 5000, 10000, 50000, 100000, 500000, 1000000, 2000000, 5000000 | Section 1.3 |

---

## 7. Alerting Recommendations

### 7.1 Critical

| Alert | Condition | Action |
|-------|-----------|--------|
| Gateway down | `/health` unreachable for 60s | Page on-call |
| Event stall | `rate(ws_events_total[5m]) == 0` for 5m | Investigate node |
| Memory spike | `process_resident_memory_bytes > 3GB` | Check client count, possible leak |

### 7.2 Warning

| Alert | Condition | Action |
|-------|-----------|--------|
| High drop rate | `rate(ws_dropped_total[5m]) > 100` | Slow client investigation |
| Buffer near-full | `broadcast_queue_usage_pct > 80` | Capacity planning |
| High finalization latency | `finalize_latency_ms p99 > 3000` | Network investigation |
| Connection churn | `rate(ws_disconnect_total[5m]) > 10` | Client stability check |

---

## 8. Measurement Infrastructure

### 8.1 Prometheus Scrape

```yaml
scrape_configs:
  - job_name: 'monad-gateway'
    scrape_interval: 15s
    static_configs:
      - targets: ['gateway:8443']
    metrics_path: '/metrics'
```

### 8.2 Grafana Dashboard Panels

Recommended panels:

1. **Active connections** — `ws_active_connections` gauge
2. **Event throughput** — `rate(ws_events_total[1m])`
3. **Drop rate** — `rate(ws_dropped_total[1m])`
4. **Finalization latency** — `histogram_quantile(0.99, rate(finalize_latency_ms_bucket[5m]))`
5. **Publish latency** — `histogram_quantile(0.99, rate(event_publish_latency_ns_bucket[5m]))`
6. **Resume mode distribution** — `rate(resume_delta_total[5m])` vs `rate(resume_snapshot_total[5m])`
7. **Buffer usage** — `broadcast_queue_usage_pct`
8. **Memory** — `process_resident_memory_bytes`
