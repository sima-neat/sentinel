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

Sentinel maps the PVT controller through `/dev/mem`, triggers each channel, and
converts its raw 12-bit-style reading using:

```text
temperature_C = raw × 698.9 / 4096 - 283.0
```

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

If RTSN sampling fails, Sentinel logs the failure and disables that RTSN
sampler for the remainder of the daemon process. Restart the daemon after
correcting `/dev/mem`, permission, or hardware-access problems.

## Board and Ethernet sensors

| Display name | Source | Physical interpretation |
| --- | --- | --- |
| `LM96-1` | `lm96163` `temp1_input` | SOM top-side board temperature from the monitoring IC's internal sensor. |
| `LM96-2` | `lm96163` `temp2_input` | SOM bottom-side temperature from a diode on the bottom of the SOM, read through the monitoring IC. |
| `ETH-1` | Ethernet/MDIO `temp1_input` | Temperature reported by the Ethernet hwmon device. The exact physical component is not yet confirmed. |

Linux hwmon reports millidegrees Celsius; Sentinel divides the value by 1000.
The LM96163 directory is resolved by driver name because `hwmonN` numbering can
change. The displayed example paths are not stable identifiers.

## Colors and thresholds

- Cyan: below 45 C
- Green: 45 C to below 70 C
- Red/warning: 70 C to below 85 C
- Bold red/critical: 85 C or higher

These are Sentinel presentation thresholds. They do not replace device
datasheet limits, thermal-trip behavior, or board-specific operating
requirements.
