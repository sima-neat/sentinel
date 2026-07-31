# Power panel

Sentinel samples PMBus `POUT` registers in a dedicated thread. The default
interval is 100 ms. The normal cache interval is slower, so the UI publishes
the latest accumulated state rather than displaying every raw PMBus sample.

## Headline fields

| Field | Calculation |
| --- | --- |
| Current | Sum of rails successfully read during the latest PMBus pass that had at least one successful rail. |
| Session average | Arithmetic mean of all valid total samples since daemon start. A pass is considered valid when at least one rail succeeds. |
| Session peak | Maximum valid total sample since daemon start. |
| Duration | Elapsed monotonic time since the power accumulator started. |
| Valid samples | PMBus passes in which at least one configured rail was read. |
| Failed samples | PMBus passes in which no configured rail was read. |

All session statistics reset when the daemon restarts.

## Collector states

- **ACTIVE**: at least one valid sample exists, the latest pass had data, and
  there is no current rail error.
- **DEGRADED**: valid data exists, but the latest pass failed completely or at
  least one rail reported an error.
- **UNAVAILABLE**: no valid total has been collected.

During a partial failure, Current, Average, and Peak totals can contain fewer
rails than a healthy sample. If a pass fails completely, Current and all rail
Power values retain their last successful readings; Failed samples and rail
Errors increase, and the state becomes DEGRADED. Do not interpret retained
values as measurements from the failed pass or compare partial and complete
totals as if they measured the same boundary.

## Rail table fields

- **Power** is the most recent successful reading for that rail. If a later
  read fails, this value remains at the last success while Errors increments.
- **Samples** counts successful readings for that rail since daemon start.
- **Errors** counts failed page-select or PMBus-register reads for that rail.

For a partially successful pass, the total uses only rails that succeeded in
that pass while each rail row retains its last successful value. For a
completely failed pass, both the total and rail rows retain earlier values.
Consequently, summing visible rail rows can differ from Current during a
partial failure, and neither value is fresh after a complete failure.

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
