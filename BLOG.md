# Monad Execution Events Gateway

## Ne Bu?

Monad full node'un EVM'i calistirirken urettigi execution event'lerini gercek zamanli olarak WebSocket uzerinden stream eden bir Rust sunucusu. Standart Ethereum JSON-RPC'de goremeyecegin seyleri gosterir: her block'un consensus asamalarini, her transaction'in ic call frame'lerini, storage slot erisimlerini, paralel execution contention metriklerini.

---

## Mimari (Buyuk Resim)

```
Monad Full Node
  └─ EVM Execution Engine
       └─ mmap'd hugepage ring buffer'a event yazar (zero-copy)
            └─ Gateway bu ring'i okur (microsecond polling)
                 └─ Enrichment (lifecycle, TPS, contention)
                      └─ Broadcast channel (1M kapasite, fan-out)
                           ├─ WebSocket client 1 (filter + bounded channel)
                           ├─ WebSocket client 2
                           └─ WebSocket client N
```

Gateway, validator'in shared memory'sinden dogrudan okur. Kernel transition yok, disk I/O yok. Event ring'den okunan C struct'lar Rust struct'larina donusturulur, JSON serialize edilir ve bagli tum client'lara broadcast edilir.

---

## Kod Yapisi

```
gateway/
  src/
    bin/
      gateway.rs                 → CLI entry point (clap: --server-addr, --event-ring-path,
                                    --heartbeat-interval, --heartbeat-timeout)
    lib/
      mod.rs                     → Module declarations
      event_listener.rs          → Ring buffer reader (mmap polling loop)
      server.rs                  → Axum HTTP/WS server (~1500 satir, ana dosya)
      serializable_event.rs      → C FFI struct → JSON-safe Rust struct donusumu
      block_lifecycle.rs         → Block state machine (Proposed→Verified)
      contention_tracker.rs      → Per-block paralel execution analizi
      event_filter.rs            → Server-side event filtering engine
      top_k_tracker.rs           → Space-Saving algorithm (top-K access tracking)
      metrics.rs                 → Prometheus counter/gauge/histogram tanimlari

sdk/typescript/                  → npm: monad-execution-events
sdk/python/                      → pip: monad-execution-events
webhook-relay/                   → HTTP POST sidecar
ops/
  prometheus/prometheus.yml      → Hazir scrape config
  grafana/gateway-dashboard.json → 15 panellik Grafana dashboard
docs/                            → API, Events, Architecture, Wire, Spec, SLO, Deployment
```

---

## Katman Katman Ne Yapiyor

### 1. Event Listener (`event_listener.rs`)

Ring buffer'i mmap ile acar, poll() loop'unda yeni event bekler. Her event bir `ExecEvent` C union'i — bunu `EventData` Rust struct'ina parse eder:

```rust
pub struct EventData {
    pub event_name: EventName,     // 25 farkli tip
    pub block_number: Option<u64>,
    pub txn_idx: Option<usize>,
    pub txn_hash: Option<[u8; 32]>,
    pub payload: ExecEvent,        // C FFI union
    pub seqno: u64,                // Ring'deki sira numarasi
    pub timestamp_ns: u64,         // Nanosecond precision
}
```

Bu struct `tokio::sync::mpsc` channel'a gonderilir → server tarafi alir.

---

### 2. Event Forwarder (`server.rs:run_event_forwarder`)

`event_listener`'dan gelen raw event'leri alip zenginlestiren async task. Her event icin:

**a) Block Lifecycle Tracking:**
- `BlockStart` → yeni block kaydi olusturur (stage: Proposed)
- `BlockQC` → stage'i Voted'a ilerletir
- `BlockFinalized` → Finalized
- `BlockVerified` → Verified (terminal)
- `BlockReject` → Rejected (terminal)
- Her geciste `BlockLifecycleUpdate` mesaji uretir (`from_stage`, `to_stage`, `block_age_ms`, `time_in_previous_stage_ms`)

**b) TPS Hesaplama:** 2.5 block'luk rolling window. Her `BlockEnd`'de `get_tps()` cagrilir:

```
TPS = block_1_txs + block_2_txs + (block_3_txs / 2)
```

~1 saniyalik pencere (400ms block time x 2.5).

**c) Contention Analysis:** Her block icin:
- Hangi storage slot'lari birden fazla transaction tarafindan erisildi?
- `parallel_efficiency_pct`: Toplam TX wall time / block wall time
- `contention_ratio`: Cekismeli slot / toplam slot
- Top contended contracts + contract co-access graph

**d) Top-K Access Tracking:** Space-Saving algoritmasi ile en cok erisilen account ve storage slot'lari takip eder. 5 dakikada bir reset.

**e) Broadcast:** Her zenginlestirilmis event'e monotonic `server_seqno` atanir, JSON'a serialize edilir, ring buffer'a kaydedilir (cursor resume icin), ve `broadcast::channel`'a gonderilir.

---

### 3. WebSocket Server (`server.rs`)

Axum framework uzerinde 5 WebSocket channel + 7 REST endpoint:

**WebSocket Channels:**

| Channel | Ne alirsin | Kimin icin |
|---------|------------|------------|
| `/v1/ws` | Her sey (25 event tipi + TPS + contention + lifecycle + top accesses) | Full firehose tuketicileri |
| `/v1/ws/blocks` | Block event'leri + TPS + lifecycle | Block explorer, dashboard |
| `/v1/ws/txs` | Transaction event'leri (header, output, log, call frame) | MEV bot, indexer |
| `/v1/ws/contention` | Sadece contention data | Analytics |
| `/v1/ws/lifecycle` | Sadece block stage gecisleri | Infra monitoring |

Her channel'in default subscription'i var. Client baglantiktan sonra JSON mesaj gondererek filtre daraltabilir:

```json
{
  "subscribe": {
    "events": ["TxnLog"],
    "min_stage": "Finalized",
    "filters": [{
      "event_name": "TxnLog",
      "field_filters": [
        {"field": "address", "filter": {"values": ["0xUniswapV3Pool"]}}
      ]
    }]
  }
}
```

**REST Endpoints:**

| Endpoint | Ne doner |
|----------|----------|
| `/v1/tps` | Anlik TPS |
| `/v1/contention` | Son block'un contention verisi |
| `/v1/status` | Gateway durumu + seqno window |
| `/v1/blocks/lifecycle` | Aktif + son block'larin lifecycle'i |
| `/v1/blocks/:number/lifecycle` | Tek block detayi |
| `/metrics` | Prometheus metrikleri (12 seri) |
| `/health` | Saglik kontrolu (30s event yoksa process exit) |

---

### 4. Per-Client Pipeline

Her WebSocket baglantisi icin:

```
Broadcast channel
  → Subscription filter (event tipi + field filter + min_stage)
  → Bounded mpsc channel (4096 kapasite)
  → Dedicated sender task → WebSocket frame
```

Backpressure mekanizmasi:
- Channel dolu → mesaj DROP edilir (block etmez)
- Her 1,000 drop'ta warning log
- 10,000 drop → client disconnect
- Bu sayede yavas bir consumer digerlerini etkilemez

---

### 5. Hello Handshake

Her WebSocket baglantisinda server ilk frame olarak `Hello` mesaji gonderir. Negotiation yok — tek yonlu deklarasyon:

```json
{
  "server_seqno": 0,
  "Hello": {
    "wire_version": 1,
    "server_version": "0.1.0",
    "features": ["lifecycle", "contention", "resume", "heartbeat", "stage_filter"],
    "limits": {
      "resume_buffer_size": 100000,
      "client_send_buffer": 4096,
      "slow_client_drop_limit": 10000,
      "heartbeat_interval_secs": 30,
      "heartbeat_timeout_secs": 60
    }
  }
}
```

`features`: Server'in destekledigi ozellikler. `limits`: Operasyonel parametreler — client bunlari okuyarak reconnect stratejisini ayarlayabilir (ornegin `resume_buffer_size`'a bakarak cursor window'u tahmin eder).

Client cevap vermezse sorun degil. Isterse bilgilendirme amacli Hello gonderebilir:

```json
{"hello": {"wire_version": 1, "client_name": "my-bot", "client_version": "1.0.0"}}
```

---

### 6. Heartbeat

Server, konfigüre edilebilir aralıklarla WebSocket Ping frame gonderir:

| Parametre | Default | CLI arg |
|-----------|---------|---------|
| Ping araligi | 30 saniye | `--heartbeat-interval` |
| Liveness timeout | 60 saniye | `--heartbeat-timeout` |

Timeout suresi icinde client'tan hicbir aktivite (Pong veya herhangi bir mesaj) gelmezse baglanti kapatilir.

---

### 7. Cursor Resume

Her wire mesaji `server_seqno` tasir. Client disconnect olup tekrar baglandiginda:

```
ws://host:8443/v1/ws?resume_from=12345
```

Server, 100K entry'lik ring buffer'dan `seqno > 12345` olan mesajlari pre-serialized JSON olarak tekrar gonderir. Re-serialize yok, zero-cost replay.

Iki mod:
- `resume`: Cursor gecerli, kayip mesaj yok
- `snapshot`: Cursor cok eski, anlik state snapshot gonderilir (TPS, contention, aktif block lifecycle'lari)

```
Connect:    ws://host:8443/v1/ws
            <- {"server_seqno":0, "Hello":{...}}
            <- {"server_seqno":0, "Resume":{"mode":"snapshot"}}
            <- {"server_seqno":5, "Events":[...]}
            ...disconnect at seqno 42...

Reconnect:  ws://host:8443/v1/ws?resume_from=42
            <- {"server_seqno":0, "Hello":{...}}
            <- {"server_seqno":42, "Resume":{"mode":"resume"}}
            <- {"server_seqno":43, ...}  // picks up where you left off
```

---

### 8. Block Lifecycle State Machine (`block_lifecycle.rs`)

```
Proposed → Voted → Finalized → Verified
    └────────────────────────────→ Rejected (herhangi asamadan)
```

- **Monotonic enforcement**: Geri gecis yapilamaz (Finalized → Voted reject edilir)
- **Stage atlama izni var**: Proposed → Finalized, eger gateway gec basladiysa
- Her geciste nanosecond-precision timing: `block_age_ms`, `time_in_previous_stage_ms`, `execution_time_ms`
- Terminal block'lar (Verified/Rejected) completed queue'ya tasinir, max 128 tutulur
- `number_to_hash` mapping ile block number → B256 hash cozumleme

---

### 9. Event Filter Engine (`event_filter.rs`)

Ic ice filter yapisi:

```
EventFilter
  └─ Vec<EventFilterSpec>     (OR logic aralarinda)
       └─ event_name: EventName
       └─ Vec<FieldFilter>    (AND logic aralarinda)
            └─ field: "address" | "txn_index" | "log_index" | "topics"
            └─ FilterValue:
                 ├─ Range { min, max }
                 ├─ Exact { values: HashSet }
                 └─ Prefix { values: Vec }
```

**Restricted mode:** Default'ta `restricted_filters.json` dosyasindan filtre yuklenir — sadece izin verilen event tipleri stream edilir. `ALLOW_UNRESTRICTED_FILTERS=1` ile tumu acilir.

---

### 10. Contention Tracker (`contention_tracker.rs`)

Her block icin paralel execution analizi:

- `BlockStart` → state reset
- Her `StorageAccess` → `(account, slot)` → `HashSet<txn_idx>` (hangi tx'ler bu slot'a eristi)
- `BlockEnd` → hesaplama:
  - `contended_slot_count`: 2+ TX'in eristigi slot sayisi
  - `parallel_efficiency_pct`: `1 - (block_wall_time / total_tx_time) x 100`
  - `top_contended_slots`: En cok cekismeli 20 slot
  - `contract_edges`: Ayni slot'u paylasan contract ciftleri (co-access graph)

---

### 11. Operasyonel Yapi

Gateway, trusted/operator-controlled ortamlar icin tasarlandi. Per-IP baglanti limiti veya subscribe limiti **yoktur**. Deployment seviyesinde koruma (reverse proxy, firewall, auth) operatorun sorumlulugundadir.

**Graceful Shutdown:**
- SIGINT veya SIGTERM sinyali
- `watch` channel ile tum WebSocket handler'lara shutdown bildirimi
- Her handler event loop'undan cikar, WebSocket'i kapatir
- Sender task'lar kalan mesajlari drain eder

**Health Check:**
- 10s event yoklugu → `"degraded"` status
- 30s event yoklugu → process exit (container restart tetikler)

---

### 12. Prometheus Metrikleri (`metrics.rs`)

12 metrik serisi, hepsi `/metrics` endpoint'inde Prometheus text format'inda:

**Gauge'lar:**
- `ws_active_connections` — Anlik WebSocket baglanti sayisi
- `broadcast_queue_usage` — Resume ring buffer'daki entry sayisi
- `broadcast_queue_usage_pct` — Ring buffer doluluk yuzdesi

**Counter'lar:**
- `ws_events_total` — Toplam broadcast edilen event
- `ws_dropped_total` — Backpressure nedeniyle drop edilen mesaj
- `ws_disconnect_total` — Toplam disconnect
- `resume_delta_total` — Basarili cursor resume
- `resume_snapshot_total` — Snapshot fallback
- `ws_heartbeat_timeout_total` — Heartbeat timeout ile disconnect

**Histogram'lar:**
- `event_publish_latency_ns` — Ring'den broadcast'e gecen sure (SLO: p99 < 5ms)
- `ws_send_latency_ns` — WebSocket'e tek mesaj yazma suresi (SLO: p99 < 2ms)
- `finalize_latency_ms` — Proposed → Finalized suresi (SLO: p99 < 2000ms)

Hazir Grafana dashboard: `ops/grafana/gateway-dashboard.json` (15 panel, 4 satir).
Hazir Prometheus scrape config: `ops/prometheus/prometheus.yml`.

---

## SDK'lar + Sidecar

**TypeScript SDK (`monad-execution-events`):**
- Auto-reconnect (exponential backoff + jitter)
- Cursor resume otomatik
- Heartbeat detection (timeout → reconnect)
- `waitForResume()` — resume mode dogrulama
- Typed events

**Python SDK (`monad-execution-events`):**
- asyncio/websockets tabanli
- Decorator API: `@client.on("lifecycle")`
- Ayni auto-reconnect + cursor resume

**Webhook Relay (`monad-webhook-relay`):**
- Gateway'e WS ile baglanir
- Event'leri HTTP POST ile N hedefe fan-out
- At-least-once delivery (retry with backoff)
- `X-Gateway-Seqno` header ile dedup

---

## 25 Event Tipi

| Kategori | Event'ler |
|----------|-----------|
| Block lifecycle | `BlockStart`, `BlockEnd`, `BlockQC`, `BlockFinalized`, `BlockVerified`, `BlockReject` |
| Block perf | `BlockPerfEvmEnter`, `BlockPerfEvmExit` |
| Txn lifecycle | `TxnHeaderStart`, `TxnAccessListEntry`, `TxnAuthListEntry`, `TxnHeaderEnd`, `TxnEvmOutput`, `TxnEnd`, `TxnReject` |
| Txn perf | `TxnPerfEvmEnter`, `TxnPerfEvmExit` |
| Txn detail | `TxnLog`, `TxnCallFrame`, `NativeTransfer` (virtual) |
| State access | `AccountAccessListHeader`, `AccountAccess`, `StorageAccess` |
| Error | `RecordError`, `EvmError` |

---

## Computed Metrics (Gateway Uretimi)

Event ring'den gelmeyen, gateway'in kendisinin hesapladigi veriler:

| Metrik | Ne | Nasil |
|--------|----|-------|
| `TPS` | Transactions per second | 2.5 block rolling window |
| `ContentionData` | Paralel execution analizi | Per-block storage slot intersection |
| `TopAccesses` | En cok erisilen account/slot | Space-Saving algoritmasi, 5dk window |
| `Lifecycle` | Block stage gecisleri | State machine + nanosecond timing |

---

## Standart RPC / WSS'den Farki

| | JSON-RPC | Bu Gateway |
|---|----------|-----------|
| **Veri kaynagi** | Finalize olmus chain state | Validator'in in-process event ring'i (shared memory) |
| **Granularite** | Block header, receipt, log | 25 event tipi: call frame, storage read, EVM timing |
| **Consensus gorunurlugu** | Yok — block'u finality'den sonra gorursun | Proposed → Voted → Finalized → Verified (ms timing ile) |
| **Contention** | Yok | Per-block paralel verimlilik, cekismeli slot sayisi, contract hotspot'lari |
| **Filtreleme** | `eth_getLogs` address/topic | Event tipi + field-level + stage-aware (`min_stage: "Finalized"`) |
| **Reconnection** | Client tekrar sorgular, dedup yapar | Server-side cursor resume: `?resume_from=<seqno>`, 100K entry ring buffer |
| **Latency** | Block seviyesi (uretimden sonra) | Sub-block: validator her transaction'i execute ettikce stream |

---

Kisacasi: Monad'in EVM'inin ic dunyasini gercek zamanli acan, production-grade bir WebSocket gateway. Standart RPC'nin vermeyecegi her seyi veriyor.
