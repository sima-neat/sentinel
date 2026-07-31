# Sentinel documentation

Sentinel collects Modalix DevKit telemetry in a background daemon, writes an
atomic JSON cache, and renders that cache through terminal reports. These pages
explain what every panel reports, where each value comes from, how it is
calculated, and what its limitations are.

## Operations-view panels

| Panel | Documentation | Purpose |
| --- | --- | --- |
| Overview | [Overview panel](panels/overview.md) | Headline thermal, power, CPU, memory, MLA-memory, storage, and network health. |
| Thermal | [Thermal panel](panels/thermal.md) | On-die RTSN/PVT sites and board-level hardware-monitor readings. |
| Power | [Power panel](panels/power.md) | PMBus rail power, totals, session average/peak, and collector health. |
| System | [System panel](panels/system.md) | Aggregate/per-core CPU, load average, Linux memory, MLA allocations, and processes. |
| Storage/Net | [Storage and network panel](panels/storage-network.md) | eMMC/NVMe capacity and I/O plus aggregate network traffic. |

## Reports and collection behavior

- [Measurement model](measurement-model.md) explains collector cadence, cache
  history, chart semantics, thresholds, stale data, unavailable values, and
  reset behavior.
- [Reports and JSON export](reports.md) documents the cache schema and the
  `table`, `export`, `sensors`, and `status` commands.

## Documentation integration contract

This directory is the canonical detailed Sentinel documentation. Pages use
relative Markdown links and do not depend on a particular documentation-site
generator. A consuming repository can import the directory as a subtree and
wrap it with site-specific navigation or front matter without rewriting the
content.

When a panel, metric key, unit, threshold, calculation, or cache field changes,
update the corresponding page in the same change.
