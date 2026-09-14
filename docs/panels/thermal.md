# Thermal panel

The Thermal panel reports 17 temperature channels:

- 14 on-die RTSN/PVT channels inside the Modalix SoC;
- two SOM board-temperature channels reported by an LM96163-compatible
  hardware-monitoring device;
- one Ethernet/MDIO hardware-monitor channel.

All values are degrees Celsius. They are point measurements at sensor
locations, not ambient-air temperature and not an average die temperature.

## Thermal max

The top chart selects the maximum finite temperature across every thermal
metric for each cache sample. The hottest sensor may change between samples.
Use the individual sensor charts to identify the source.

## On-die RTSN/PVT sensors

Sentinel reads the Linux hwmon device named `simaai_modalix_thermal_sensor`.
Its `temp1_input` through `temp14_input` map to Sentinel's `rtsn_0` through
`rtsn_13`. The kernel supplies millidegrees Celsius, which Sentinel divides by
1000; the kernel owns measurement mode, sequencing, and calibration.

Sentinel never maps `/dev/mem` or programs the PVT controller. Direct register
access races the kernel driver and can disrupt thermal protection. There is
no direct-register fallback on older images without the hwmon driver: on-die
temperatures are unavailable until the kernel exposes the sensor interface.
Keep kernel thermal protection enabled.

The SoC has seven physical sites. Each site is represented by an Alert-channel
RTSN and a Trip-channel RTSN at the same location:

| Location | Alert channel | Trip channel |
| --- | --- | --- |
| MLA quadrant Q0 | `MLA-0` | `MLA-7` |
| MLA quadrant Q1 | `MLA-1` | `MLA-8` |
| MLA quadrant Q2 | `MLA-2` | `MLA-9` |
| MLA quadrant Q3 | `MLA-3` | `MLA-10` |
| APU | `APU-4` | `APU-11` |
| CVU | `CVU-5` | `CVU-12` |
| TOP, near PCIe/Ethernet | `TOP-6` | `TOP-13` |

The pair does not represent two different temperature limits in the displayed
value. It represents two sensor channels feeding different hardware threshold
paths at the same site. Small differences between the pair are expected.

A missing sensor or failed read is displayed as unavailable (JSON `null`),
not zero or the previous sample. Sentinel retries on the next collection and
rediscovers hwmon devices each time, including after driver rebinding.

## Board and Ethernet sensors

| Display name | Source | Physical interpretation |
| --- | --- | --- |
| `LM96-1` | `lm96163` `temp1_input` | SOM top-side board temperature from the monitoring IC's internal sensor. |
| `LM96-2` | `lm96163` `temp2_input` | SOM bottom-side temperature from a diode on the bottom of the SOM, read through the monitoring IC. |
| `ETH-1` | `a800000ethernetmdio000` `temp1_input` | Temperature reported by the Ethernet hwmon device. The exact physical component is not yet confirmed. |

Linux hwmon reports millidegrees Celsius; Sentinel divides the value by 1000.
All directories are resolved by their hwmon `name` because `hwmonN` numbering
can change. Sentinel does not fall back to numbered directories: an unrelated
chip must never be reported as a board or Ethernet sensor.

## Colors and thresholds

- Cyan: below 45 C
- Green: 45 C to below 70 C
- Red/warning: 70 C to below 85 C
- Bold red/critical: 85 C or higher

These are Sentinel presentation thresholds. They do not replace device
datasheet limits, thermal-trip behavior, or board-specific operating
requirements.
