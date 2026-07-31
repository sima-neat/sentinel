# SiMa.ai Sentinel

Sentinel is a Rust-based Modalix DevKit metrics collector. It monitors board
sensors and system utilization in the background, then exposes a low-overhead
CLI for live inspection.

The package installs:

- `simaai-sentinel`: a CLI that reads the daemon cache and renders live tables,
  terminal line graphs, and sensor reference information.
- `simaai-sentinel daemon`: a systemd-managed sampling daemon that reads board
  thermal sensors, CPU load, Linux memory, MLA allocator memory, disk usage,
  disk IO, network IO, and Modalix PMBus power. It keeps a timestamped cache
  under `/run/simaai-sentinel`.

## Install with sima-cli

```bash
sima-cli neat install sentinel
```

The installer must run on a Modalix DevKit because the daemon reads hardware
sensors through `/dev/mem`, Linux `hwmon`, and `/dev/i2c-*`. The package
installs a prebuilt aarch64 binary and does not require a Rust toolchain on the
DevKit.

## Usage

```bash
simaai-sentinel                 # open the terminal operations view
simaai-sentinel table           # continuously redraw a color-coded table
simaai-sentinel table --once    # print one table snapshot
simaai-sentinel export          # print the daemon cache as JSON
simaai-sentinel sensors         # explain each collected metric
simaai-sentinel status          # daemon/cache status
simaai-sentinel checkpoint --name baseline
simaai-sentinel checkpoint --stop
simaai-sentinel runs list
```

Agents and local automation can use the daemon's Unix-socket JSON API for live
telemetry and checkpoint control. See [Local agent API](docs/api.md).

## Documentation

Detailed explanations of every operations-view panel and exported report field
are available in the [Sentinel documentation](docs/README.md):

- [Overview](docs/panels/overview.md)
- [Thermal](docs/panels/thermal.md)
- [Power](docs/panels/power.md)
- [System](docs/panels/system.md)
- [Storage/Net](docs/panels/storage-network.md)
- [Sampling, history, and status semantics](docs/measurement-model.md)
- [JSON export and command reports](docs/reports.md)
- [Checkpoint capture and run comparison](docs/run-comparison.md)

## JSON export

Use `simaai-sentinel export` when another script or application needs the
latest sensor data. The command reads the daemon cache and prints JSON to
stdout:

```bash
simaai-sentinel export > sentinel-cache.json
```

The exported document includes metric definitions, the latest sample, recent
sample history, process summaries, power status, and daemon errors. See
[Reports and JSON export](docs/reports.md) for the schema and field semantics.

## Service

```bash
sudo systemctl status simaai-sentinel
sudo systemctl restart simaai-sentinel
sudo journalctl -u simaai-sentinel -f
```

The daemon cache is intentionally kept in `/run`, so it is refreshed on reboot
and does not create persistent log growth by default.
