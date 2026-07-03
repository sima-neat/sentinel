# SiMa.ai Sentinel

Sentinel is a Rust-based Modalix DevKit metrics collector. It monitors board
sensors and system utilization in the background, then exposes a low-overhead
CLI for live inspection.

The package installs:

- `simaai-sentinel`: a CLI that reads the daemon cache and renders live tables,
  terminal line graphs, and sensor reference information.
- `simaai-sentinel daemon`: a systemd-managed sampling daemon that reads board
  thermal sensors, CPU load, Linux memory, MLA allocator memory, disk usage,
  disk IO, and network IO. It keeps a timestamped cache under
  `/run/simaai-sentinel`.

## Install with sima-cli

```bash
sima-cli neat install sentinel
```

The installer must run on a Modalix DevKit because the daemon reads hardware
sensors through `/dev/mem` and Linux `hwmon`. The package installs a prebuilt
aarch64 binary and does not require a Rust toolchain on the DevKit.

## Usage

```bash
simaai-sentinel                 # open the terminal operations view
simaai-sentinel table           # continuously redraw a color-coded table
simaai-sentinel table --once    # print one table snapshot
simaai-sentinel sensors         # explain each collected metric
simaai-sentinel status          # daemon/cache status
```

Temperature coloring:

- Cyan: below 45 C
- Green: 45 C to below 70 C
- Red: 70 C or higher
- Bold red: 85 C or higher

## Service

```bash
sudo systemctl status simaai-sentinel
sudo systemctl restart simaai-sentinel
sudo journalctl -u simaai-sentinel -f
```

The daemon cache is intentionally kept in `/run`, so it is refreshed on reboot
and does not create persistent log growth by default.

Collected system metrics include:

- CPU utilization and 1-minute load average
- Linux memory usage
- MLA memory allocation from `/dev/simaai-mem`
- eMMC disk usage and IO rate
- NVMe disk usage and IO rate when `/media/nvme` is mounted
- Aggregate non-loopback network RX/TX rate
