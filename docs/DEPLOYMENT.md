# Deployment

## Prerequisites

- Monad full node with Execution Events SDK enabled
- Event ring at `/var/lib/hugetlbfs/user/monad/pagesize-2MB/event-rings/`

## Docker

```bash
docker compose up -d
```

Port `8443`, auto-restart, event ring mounted read-only. The Docker container binds `0.0.0.0` (required for Docker networking); native builds default to `127.0.0.1`. Custom port: edit `docker-compose.yml` command to `--server-addr 0.0.0.0:9090`.

## Native Build

```bash
# Ubuntu 24.04 deps
wget https://apt.llvm.org/llvm.sh && chmod +x llvm.sh && sudo ./llvm.sh 19
sudo apt install -y clang-19 libclang-19-dev libzstd-dev libhugetlbfs-dev libbsd-dev cmake gcc g++

# Build & run
cd gateway && ./build.sh --run
```

Custom event ring path:

```bash
cargo run --release --bin gateway -- --event-ring-path /path/to/ring --server-addr 127.0.0.1:8443
```

## Configuration

### CLI Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `--server-addr` | `127.0.0.1:8443` | Listen address (use `0.0.0.0` inside Docker) |
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
  --server-addr 127.0.0.1:8443 \
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

## Monitoring

### Docker (recommended)

```bash
docker compose --profile monitoring up -d
```

This starts gateway + Prometheus (`:9095`) + Grafana (`:3001`, admin/admin) in a single command. Without the `--profile` flag, only the gateway starts.

### Prometheus

Use the ready-to-use scrape config at `ops/prometheus/prometheus.yml`:

```bash
cp ops/prometheus/prometheus.yml /etc/prometheus/conf.d/gateway.yml
```

The gateway exposes 50+ metrics at `GET /metrics` in Prometheus text format (counters, histograms, gauges).

### Grafana

Import the dashboard from `ops/grafana/gateway-dashboard.json`:

1. Open Grafana -> Dashboards -> Import
2. Upload `ops/grafana/gateway-dashboard.json`
3. Select your Prometheus datasource
4. Dashboard includes: latency SLOs, throughput, connections, resource usage, resume stats

See [SLO Definitions](slo.md) for metric descriptions and alerting recommendations.

## Release Artifacts

On tagged releases (`v*`), the CI pipeline produces:

| Artifact | Location |
|----------|----------|
| Linux binary | GitHub Releases (`gateway-linux-amd64.tar.gz`) |
| Docker images | `ghcr.io/<org>/monad-execution-gateway/gateway:<version>` |
| TypeScript SDK | `npm install monad-execution-events@<version>` |
| Python SDK | `pip install monad-execution-events==<version>` |

To trigger a release:

```bash
git tag v0.2.0
git push origin v0.2.0
```
