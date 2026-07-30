# Overview panel

The Overview panel is a summary assembled from the same metrics shown in the
detailed panels. It does not collect separate data.

## Headline charts

| Chart | Metric/calculation | Meaning |
| --- | --- | --- |
| Thermal Max | Maximum finite temperature across all current thermal metrics | Hottest reported on-die or board sensor at each cache sample. The sensor producing the maximum can change over time. |
| Current Power | `power_current_watts` | Latest valid total of PMBus rails successfully read in the corresponding power pass. See [Power](power.md) for partial-read behavior. |
| CPU | `cpu_usage_pct` | Aggregate Linux CPU busy percentage over the latest system sample window. |
| Memory | `linux_mem_used_pct` | `(MemTotal - MemAvailable) / MemTotal × 100`. |
| MLA Memory | `mla_mem_allocated_mb` | Allocated bytes reported by `/dev/simaai-mem`, converted to MiB. This is allocator usage, not MLA utilization. |
| Network | `net_rx_mbps + net_tx_mbps` | Combined aggregate receive and transmit rate for all non-loopback interfaces. Despite the key name, the displayed unit is MiB/s-equivalent, not megabits/s. |

The Thermal Max chart is useful for spotting the hottest location but should
not replace the Thermal panel: it hides which sensor is responsible and can
switch between on-die and board sensors.

## Current status table

The table reports:

- normalized one-minute CPU load;
- Linux memory used in MiB;
- MLA allocator memory in MiB;
- current board-rail power;
- eMMC and optional NVMe filesystem usage;
- aggregate receive and transmit rates.

The columns mean:

- **Metric**: short display name from the cache metric definition.
- **Group**: subsystem category.
- **Value**: latest finite cached value.
- **Status**: `normal`, `warning`, `critical`, or `unknown`, based on the
  metric's configured thresholds.
- **Trend**: normalized sparkline across retained cache history.

## Notes panel

The Notes panel displays top-level daemon errors when present. With no daemon
errors it displays usage guidance. An empty Notes panel is not proof that every
sensor is healthy; Power has its own degraded status and individual metric
values can be unavailable without becoming top-level daemon errors.
