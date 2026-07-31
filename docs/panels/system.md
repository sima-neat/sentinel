# System panel

The System panel combines Linux CPU counters, load average, memory accounting,
the SiMa MLA allocator report, and periodic process snapshots.

## CPU fields

| Field | Source and calculation |
| --- | --- |
| CPU usage | Delta of aggregate `/proc/stat` CPU time between system samples. Busy percentage is `1 - idle_delta / total_delta`; idle includes idle and I/O-wait fields. |
| CPU core N usage | Same delta calculation for each `cpuN` line in `/proc/stat`. |
| CPU load 1m | First value in `/proc/loadavg`; Linux's exponentially averaged count of runnable tasks and tasks in uninterruptible wait. |
| CPU load 1m percentage | `load1 / logical_core_count × 100`. It is a normalization for presentation and can exceed 100%. |

CPU usage is immediate busy time over the latest sample window. Load is queued
or waiting work over roughly one minute. High load with low CPU usage can
indicate I/O waits or blocked work rather than compute saturation.

The first CPU sample has no previous counter baseline and is unavailable.

## Per-core and process panels

The per-core bars show the latest `cpuN` utilization. Up/down selects a core.

The process table fields are:

| Field | Meaning |
| --- | --- |
| PID | Linux process identifier. |
| CPU | Last processor number recorded in `/proc/<pid>/stat`; it is where the process was last observed, not an affinity guarantee. |
| %CPU | Process user+system CPU-tick delta divided by elapsed time. Approximately 100% represents one fully used logical CPU. Multithreaded processes can exceed 100%. |
| RSS MB | Resident pages from `/proc/<pid>/stat` multiplied by system page size and converted to MiB. |
| Name | Process command name from `/proc/<pid>/stat`. |

Process data refreshes every five seconds. Sentinel retains only the top 64
processes globally, sorted by CPU and then RSS. When a selected core has no
retained process, the UI falls back to showing the global retained list.
Consequently, this panel is diagnostic rather than an exhaustive process
accounting report.

## Linux memory

```text
used_bytes = MemTotal - MemAvailable
used_pct   = used_bytes / MemTotal × 100
```

`MemAvailable` comes from `/proc/meminfo` and estimates memory available for
new work without swapping. This means cache that Linux can reclaim is not
automatically treated as unavailable memory.

The `MB` display is calculated using 1024 × 1024 bytes, so it is technically
MiB even though the UI label is MB.

## MLA memory

Sentinel reads `/dev/simaai-mem`, finds `Total allocated size:`, parses the
following hexadecimal byte count, and converts it to MiB.

This is MLA allocator memory currently allocated. It is not:

- MLA compute utilization;
- model FPS or latency;
- total physical DRAM use;
- a measurement of `MLA 0.68V` power.

If the device cannot be read or its text format is unrecognized, the value is
unavailable rather than zero.
