# Deployment Guide

## Prerequisites

- A running Monad validator node with the Execution Events SDK enabled
- Event ring files at `/var/lib/hugetlbfs/user/monad/pagesize-2MB/event-rings/`
- Rust 1.91+ (for native builds) or Docker

## Option 1: Docker (Recommended)

```bash
docker compose up -d
```

The gateway will:
- Bind all endpoints (WebSocket, REST, health) on port `8443`
- Mount the event ring read-only from hugepages
- Auto-restart on failure

### Custom Port

```bash
# Edit docker-compose.yml command:
command: ["--server-addr", "0.0.0.0:9090"]
```

## Option 2: Native Build

### Install Dependencies (Ubuntu 24.04)

```bash
# Install clang-19 for the Monad SDK's C23 headers
wget https://apt.llvm.org/llvm.sh && chmod +x llvm.sh && sudo ./llvm.sh 19
sudo apt install -y libclang-19-dev libzstd-dev libhugetlbfs-dev cmake gcc g++
```

### Build

```bash
cd gateway
./build.sh
```

### Run

```bash
./build.sh --run
# or directly:
cargo run --release --bin gateway -- --server-addr 0.0.0.0:8443
```

### Custom Event Ring Path

```bash
cargo run --release --bin gateway -- \
  --event-ring-path /path/to/your/event-ring \
  --server-addr 0.0.0.0:8443
```

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level (trace, debug, info, warn, error) |
| `ALLOW_UNRESTRICTED_FILTERS` | unset | Set to any value to stream all events (no filtering) |

### Event Filtering

By default, the gateway runs in **restricted mode** and only streams events defined in `restricted_filters.json`. This is suitable for public-facing deployments.

To stream all execution events (useful for bots, analytics, research):

```bash
ALLOW_UNRESTRICTED_FILTERS=1 cargo run --release --bin gateway -- --server-addr 0.0.0.0:8443
```

### Firewall

Open the gateway port:

```bash
sudo ufw allow 8443/tcp
```

## Health Monitoring

The gateway exposes a health endpoint on the same port:

```bash
curl http://localhost:8443/health
# {"success": true}
```

- Returns `false` if no events received in 10 seconds
- Process exits if no events for 30 seconds (triggers Docker restart)

Additional status endpoint:

```bash
curl http://localhost:8443/v1/status
# {"status":"healthy","block_number":56147820,"connected_clients":3,"uptime_secs":86400,"last_event_age_secs":0}
```

## Systemd Service (Alternative to Docker)

```ini
[Unit]
Description=Monad Execution Events Gateway
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/monode/gateway
ExecStart=/opt/monode/gateway/target/release/gateway --server-addr 0.0.0.0:8443
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

```bash
sudo cp monode-gateway.service /etc/systemd/system/
sudo systemctl enable --now monode-gateway
```
