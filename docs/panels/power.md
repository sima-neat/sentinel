# Power panel

Sentinel reads each rail's output power (`POUT`) from the kernel's PMBus
driver in a dedicated thread every 250 ms. The normal cache interval is
slower, so the UI publishes the latest accumulated state rather than
displaying every power reading.

## Source

Each rail is one `pout` channel of a PMBus regulator's hwmon device:
`/sys/bus/i2c/devices/<bus>-<address>/hwmon/hwmon*/power<N>_input`, in
microwatts, where `power<N>_label` names the channel (for example `pout1` for
PMBus page 0). Sentinel finds the device by its I2C address and the channel by
its label, never by `hwmonN` or channel number. The kernel driver selects the
PMBus page and reads the register under its own lock, so other programs that
read the regulators cannot corrupt Sentinel's readings.

On Modalix the regulators are IRPS5401 devices. The device tree declares them,
but the `irps5401` module does not load on its own, so the Sentinel service
runs `modprobe irps5401` before it starts. When no driver is bound, power is
unavailable and the collector error names the module to load.

## Headline fields

| Field | Calculation |
| --- | --- |
| Current | Sum of every configured rail in the latest pass. Empty when any rail could not be read. |
| Session average | Arithmetic mean of the totals of all complete passes since daemon start. A pass is complete when every rail was read. |
| Session peak | Maximum complete-pass total since daemon start. |
| Duration | Elapsed monotonic time since the power accumulator started. |
| Valid samples | Passes in which every configured rail was read. |
| Failed samples | Passes in which at least one configured rail could not be read. |

All session statistics reset when the daemon restarts.

## Collector states

- **ACTIVE**: at least one complete pass exists, and the latest pass read
  every rail.
- **DEGRADED**: a complete pass exists, but the latest pass could not read at
  least one rail.
- **UNAVAILABLE**: no complete pass has been collected.

Sentinel never shows an earlier reading as current. When a rail cannot be
read, its Power value and Current are empty until the next successful pass,
Failed samples and the rail's Errors increase, and the state becomes DEGRADED.
Average and Peak include only complete passes, so every total covers the same
rails.

## Rail table fields

- **Power** is the rail's reading from the latest pass, or empty when that
  read failed.
- **Samples** counts successful readings for that rail since daemon start.
- **Errors** counts failed reads of that rail's kernel `power<N>_input` file,
  for example while no driver is bound.

## Modalix SOM rails

| Rail | What it supplies |
| --- | --- |
| `SoC and DDR VDD` | Main SoC and DDR domain; SoC logic, memory-controller, and DDR activity contribute. |
| `HDMI 1.2V` | HDMI 1.2 V PHY and related circuitry. |
| `SoC and DDR VDDQ` | DDR I/O signaling domain between the memory controller and DDR devices. |
| `SoC and DDR VDD2H` | Additional SoC/DDR memory power domain. |
| `MLA 0.68V` | 0.68 V MLA core supply and the rail most directly associated with MLA compute. |
| `PCIe and ETH VP` | PCIe and Ethernet PHY/platform domain. |
| `SOM Platform 1.8V` | Shared 1.8 V SOM platform and peripheral circuitry. |
| `SOM Platform 3.3V` | Shared 3.3 V SOM platform and peripheral circuitry. |

`SoC and DDR VDD` and `MLA 0.68V` have a 0.25 W reporting step. The other
standard SOM rails have a 0.03125 W step. Therefore, `0.00 W` with increasing
Samples and zero Errors is a valid zero-valued register reading at the rail's
resolution, not a sensor-read failure.

The Modalix DVT profile exposes only `DVT PMIC 0x4d page0`. Its total is not
directly comparable to the eight-rail SOM total.

## Measurement boundary

Current is monitored board-rail power, not AC wall power or complete system
input power. It can exclude unmonitored rails, regulator losses, carrier-board
components, storage, fans, and host-side devices.

Likewise, model energy is not represented by `MLA 0.68V` alone. Inference also
uses DDR, SoC, and platform rails. Record the profile, rail health, Sentinel
version, workload window, and ambient/board conditions when comparing runs.
