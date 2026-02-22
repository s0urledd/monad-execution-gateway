# Deployment

## Prerequisites

- Monad full node with Execution Events SDK enabled
- Event ring at `/var/lib/hugetlbfs/user/monad/pagesize-2MB/event-rings/`

## Docker

```bash
docker compose up -d
```

Port `8443`, auto-restart, event ring mounted read-only. Custom port: edit `docker-compose.yml` command to `--server-addr 0.0.0.0:9090`.

## Native Build

```bash
# Ubuntu 24.04 deps
wget https://apt.llvm.org/llvm.sh && chmod +x llvm.sh && sudo ./llvm.sh 19
sudo apt install -y libclang-19-dev libzstd-dev libhugetlbfs-dev cmake gcc g++

# Build & run
cd gateway && ./build.sh --run
```

Custom event ring path:

```bash
cargo run --release --bin gateway -- --event-ring-path /path/to/ring --server-addr 0.0.0.0:8443
```

## Configuration

### CLI Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `--server-addr` | `0.0.0.0:8443` | Listen address |
| `--event-ring-path` | `/var/lib/hugetlbfs/.../monad-exec-events` | Path to event ring |
| `--heartbeat-interval` | `30` | Seconds between server Ping frames |
| `--heartbeat-timeout` | `60` | Seconds without client activity before disconnect |

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level |
| `ALLOW_UNRESTRICTED_FILTERS` | unset | Set to stream all events (bypasses restricted_filters.json) |

## Health

```bash
curl http://localhost:8443/health    # {"success": true}
curl http://localhost:8443/v1/status  # seqno window, clients, uptime
```

No events for 10s = degraded. No events for 30s = process exits (Docker restarts it).

## Systemd

```ini
[Unit]
Description=Monad Execution Events Gateway
After=network.target

[Service]
Type=simple
ExecStart=/opt/monode/gateway/target/release/gateway \
  --server-addr 0.0.0.0:8443 \
  --heartbeat-interval 30 \
  --heartbeat-timeout 60
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now monode-gateway
```
