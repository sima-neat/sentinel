# Measurement model

Sentinel separates data collection from display:

1. The daemon reads hardware and Linux counters.
2. It periodically writes `/run/simaai-sentinel/cache.json` using a temporary
   file followed by an atomic rename.
3. The terminal UI and reporting commands read that cache. They do not sample
   hardware directly.

This distinction matters: changing a UI `--interval` changes only how often the
cache is displayed. It does not change the hardware sampling interval.

## Default collection cadence

| Data | Default cadence | Notes |
| --- | ---: | --- |
| PMBus power | 100 ms | Collected in a dedicated thread. Latest, average, peak, rail counters, and error state are published at the next cache update. |
| Thermal sensors | Separate thermal loop | The loop sleeps for 2 seconds after each collection pass. Reading all RTSN channels also takes time, so the effective complete-pass interval is longer than 2 seconds. |
| CPU, memory, MLA memory, disk, and network | 2 seconds | Uses the daemon `--interval`; minimum accepted value is 0.5 seconds. |
| Processes | At least 5 seconds | The 5-second deadline is checked only when the normal daemon loop samples system metrics. With the default 2-second daemon interval, process data normally refreshes about every 6 seconds; a daemon interval longer than 5 seconds becomes the effective minimum. |
| Terminal UI | 0.5 seconds | Cache refresh only; it does not trigger collection. |

The daemon keeps 240 cache samples by default. At the default 2-second cache
interval, charts contain approximately eight minutes of history. `daemon
--history N` changes the number of cache samples retained.

## Header fields

- **LIVE** means the newest cache sample is less than 15 seconds old.
- **STALE** means the newest sample is at least 15 seconds old or missing.
- **age** is wall-clock time since the timestamp of the newest sample.
- **samples** is the number of cache-history entries, not the number of PMBus,
  thermal, or process samples.

A LIVE cache can still contain a missing or degraded subsystem. Use the
subsystem panel and exported status fields to distinguish cache freshness from
sensor health.

## Values and charts

- A headline or table **Value** comes from `latest.values`.
- A chart is built from that metric across the retained `samples` history.
- A collected non-finite value is serialized as `null`. Depending on the
  collector, an unavailable metric can instead be omitted from the sample
  entirely—for example, power keys are absent until their first valid reading,
  and unavailable or disabled RTSN channels are omitted. Views and export
  consumers must handle both an absent key and a `null` value; the UI displays
  these as `-`, `unknown`, or an empty chart depending on the view.
- Dynamic chart scales are presentation aids. A graph reaching the top does
  not by itself mean a hardware limit was reached.
- **Trend** sparklines normalize values to a fixed 0–100 display range. They
  show direction and relative movement, not a separate measurement.

## Status and thresholds

Metric definitions in the cache carry optional warning and critical
thresholds. The current defaults are:

| Metrics | Warning | Critical |
| --- | ---: | ---: |
| Thermal sensors | 70 C | 85 C |
| CPU aggregate and per-core usage | 80% | 95% |
| Normalized one-minute CPU load | 80% | 95% |
| Linux memory used | 80% | 90% |
| eMMC/NVMe filesystem used | 80% | 90% |

Metrics without thresholds report `normal` when available. `unknown` means no
finite value is present; it does not mean the value is zero.

In the interactive TUI, Thermal-tab line charts are green; the Overview
temperature chart uses green below 70 C, yellow from 70 C, and red from 85 C.
The legacy non-TUI table formatter uses cyan below 45 C, green from 45 C to
below 70 C, red from 70 C, and bold red from 85 C.

## Reset and persistence

The cache lives under `/run`, so it is normally cleared at reboot. History,
power session average/peak, power rail sample/error counts, and process delta
baselines reset when the Sentinel daemon restarts. Sentinel does not create a
persistent time-series database.
