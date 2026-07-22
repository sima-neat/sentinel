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
```

## JSON export

Use `simaai-sentinel export` when another script or application needs the
latest sensor data. The command reads the daemon cache and prints JSON to
stdout:

```bash
simaai-sentinel export > sentinel-cache.json
```

The exported document includes metric definitions, the latest sample, recent
sample history, process summaries, and daemon error messages. Temperature
sensor values are included in `latest.values` using the metric keys described
by the `metrics` array.

## Power monitoring

The **Power** tab shows current board power together with average and peak power
since the daemon started. Sentinel samples PMBus POUT registers every 100 ms so
short peaks are retained, then publishes aggregate values through its normal
cache interval. The recent-current chart uses the cache history.

Sentinel implements the same PMBus protocol and Modalix SOM/DVT rail profiles
as Neat Core's power telemetry without linking to the Core library. Individual
rail failures produce a degraded status while successfully read rails continue
to contribute to the total. If no rail can be read, the UI reports power as
unavailable instead of displaying zero.

Power statistics reset when the daemon restarts. The JSON export includes the
three total metrics, per-rail metrics, and a `power` status object containing
the detected profile, sample counts, read failures, and latest rail values.

## Thermal Color Coding

Temperature coloring:

- Cyan: below 45 C
- Green: 45 C to below 70 C
- Red: 70 C or higher
- Bold red: 85 C or higher

## Hardware temperature sensors

Sentinel reports 17 thermal readings in three distinct classes:

- 14 on-die RTSN/PVT readings from inside the Modalix SoC
  (`MLA-*`, `APU-*`, `CVU-*`, `TOP-*`),
- 2 SOM board-temperature readings from the LM96063 hardware-monitoring IC
  (`LM96-1`, `LM96-2`),
- 1 Ethernet/MDIO reading (`ETH-1`).

None of these is an ambient (air) temperature.

### On-die RTSN sensors (`MLA-*`, `APU-*`, `CVU-*`, `TOP-*`)

The SoC has 7 thermal sensor sites across 4 subsystems: the MLA (four
quadrants, Q0-Q3), the APU, the CVU, and TOP (near the PCIE/ETH area). Each
site needs two distinct thresholds — a low "Thermal Alert" and a high
"Thermal Trip" — and a design rule prevents connecting one RTSN to more than
one DTS Hub channel, so the Remote Temperature Sensor (RTSN) is duplicated at
each site, giving 14 RTSNs total:

- Channels 0-6 feed each site's Alert channel, which fires the early "Alert"
  interrupt.
- Channels 7-13 feed each site's Trip channel, which fires the hard "Trip".

Sentinel names each reading `<subsystem>-<channel>`, so a sensor pair such as
`MLA-0`/`MLA-7` is one Alert/Trip duplicate pair sitting at the same physical
spot (the RTSN is only 20 um x 18 um). Each reading is a local on-die point
measurement at its site, not an average over a logical region; the paired
readings measure the same location and differ only in which threshold channel
they drive.

| Subsystem | Sensor site / location | Alert sensor (ch 0-6) | Trip sensor (ch 7-13) |
| --- | --- | --- | --- |
| MLA | Q0 quadrant | `MLA-0` | `MLA-7` |
| MLA | Q1 quadrant | `MLA-1` | `MLA-8` |
| MLA | Q2 quadrant | `MLA-2` | `MLA-9` |
| MLA | Q3 quadrant | `MLA-3` | `MLA-10` |
| APU | APU | `APU-4` | `APU-11` |
| CVU | CVU | `CVU-5` | `CVU-12` |
| TOP | Near the PCIE/ETH area | `TOP-6` | `TOP-13` |

The daemon acquires these by sampling the SoC PVT controller registers
through `/dev/mem`.

### Board and Ethernet sensors (Linux hwmon)

| Sentinel sensor | Physical measurement location | Measurement method / path |
| --- | --- | --- |
| `LM96-1` | Top side of the SOM | SOM top-side board temperature, reported by the LM96063 hardware-monitoring IC (Linux `lm96163` hwmon driver, `temp1_input`, e.g. `/sys/class/hwmon/hwmon2/temp1_input`). This is a board-temperature measurement, not ambient or processor-junction temperature. |
| `LM96-2` | Bottom side of the SOM | SOM bottom-side board temperature. A diode physically located on the bottom side of the SOM senses the temperature; the diode is electrically connected to, read by, and reported through the LM96063 IC (`temp2_input`, e.g. `/sys/class/hwmon/hwmon2/temp2_input`). |
| `ETH-1` | Ethernet/MDIO device | `ETH/MDIO temp1` Linux hwmon reading (`temp1_input`, e.g. `/sys/class/hwmon/hwmon0/temp1_input`). The exact Ethernet component and its board location are not yet confirmed from hardware documentation; this is tracked in [issue #5](https://github.com/sima-neat/sentinel/issues/5). |

For hwmon entries, the daemon reads `temp*_input` values from sysfs
(millidegrees Celsius, converted to C) and resolves the LM96063 by its
`lm96163` driver name rather than relying on a fixed `hwmonN` number; the
paths above are examples.

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
- Current, session-average, and session-peak board power
- Per-rail PMBus POUT readings and read-error counts
