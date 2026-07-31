# Reports and JSON export

All Sentinel commands read the daemon cache unless the `daemon` command is
running the collectors.

## Commands

| Command | Output |
| --- | --- |
| `simaai-sentinel` | Interactive five-panel terminal operations view. |
| `simaai-sentinel table` | Continuously refreshed metric table with current value, unit, and threshold status. |
| `simaai-sentinel table --once` | One table snapshot suitable for logs. |
| `simaai-sentinel export` | Complete cache document as formatted JSON. |
| `simaai-sentinel sensors` | Metric names, groups, units, thresholds, and collector-provided descriptions. |
| `simaai-sentinel status` | Sentinel version, cache timestamp, latest sample time/age, history size, metric count, and top-level daemon errors. |

Use `--cache PATH` to read or write a non-default cache. UI/table refresh
intervals affect only display. `daemon --interval SEC` changes the normal
system/cache cadence; PMBus, thermal, and process collectors retain their own
cadences.

## Cache document

`simaai-sentinel export` prints this top-level structure:

| Field | Meaning |
| --- | --- |
| `schema` | Cache schema version. Currently `1`. |
| `version` | Sentinel package version that wrote the cache. |
| `updated_at` | UTC time at which the cache payload was written. |
| `metrics` | Definitions for all metrics available when the daemon started. |
| `latest` | Newest cache sample, or `null` before a sample exists. |
| `samples` | Retained ring-buffer history in chronological order. |
| `processes` | Latest retained process snapshot, refreshed every five seconds. |
| `power` | Power collector status and per-rail counters, or `null` for an older/incompatible cache. |
| `errors` | Top-level daemon messages. Power rail errors normally live under `power.last_error`. |

## Metric definitions

Each `metrics` entry contains:

| Field | Meaning |
| --- | --- |
| `key` | Stable machine-readable name used in `Sample.values`. |
| `label` | Long human-readable name. |
| `short` | Compact UI/table label. |
| `group` | Subsystem category. |
| `unit` | Display unit. |
| `description` | Collector-provided measurement summary. |
| `warn` | Optional warning threshold. |
| `critical` | Optional critical threshold. |

Consumers should join sample values to definitions by `key`, not by label or
array position.

## Samples

Each sample has:

- `timestamp`: UTC collection/cache timestamp;
- `values`: mapping of metric key to a number or `null`.

`null` means the collector did not produce a finite value. It must not be
interpreted as zero. A key may also be absent when a subsystem was not
configured, such as NVMe when `/media/nvme` was not mounted at daemon startup.

## Process records

Each process record contains `pid`, `name`, `cpu_pct`, `rss_mb`, and optional
`cpu_core`. See the [System panel](panels/system.md) for calculation and
selection limitations.

## Power status

The `power` object contains:

| Field | Meaning |
| --- | --- |
| `profile` | Detected rail profile, such as `modalix_som` or `modalix_dvt`. |
| `sample_interval_ms` | Configured PMBus sampling interval. |
| `duration_seconds` | Time since power accumulation started. |
| `valid_samples` | Passes with at least one successful rail. |
| `failed_samples` | Passes with no successful rails. |
| `last_sample_valid` | Whether the latest PMBus pass read at least one rail. |
| `last_error` | Combined error text for rails that failed in the latest pass, or `null`. |
| `rails` | Per-rail label, metric key, last successful watts, successful sample count, and error count. |

See the [Power panel](panels/power.md) before using these fields for automated
comparison; partial rail reads affect total comparability.

## Atomicity and retention

The daemon writes a temporary file, flushes it, and renames it over the cache,
so readers should see either the previous complete payload or the new complete
payload. The cache is a current-state and short-history report, not a durable
audit log. Export it externally when persistent evidence is required.
