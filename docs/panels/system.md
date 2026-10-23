# System panel

The System panel combines Linux CPU counters, load average, memory accounting,
SiMa MLA and EV74 memory telemetry, contiguous-memory (CMA) usage, and
periodic process snapshots.

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

On Platform 3.0, the presence of `/dev/dma_heap/simaai,dms` selects DMA-BUF
telemetry. Sentinel reads `/sys/kernel/debug/dma_buf/bufinfo` and sums the
objects exported by the MLA heaps, `mla` or `simaai,dms`, plus shared-pool
buffers that only the MLA uses, such as an input the CPU prepares with no EV74
stage or an MLA allocation that spills from the MLA heap into CMA, counting each
DMA-BUF inode once. A shared-pool buffer that EV74 (`cvu`) also uses is not MLA
memory: the preprocessed inputs EV74 writes for the MLA count as EV74 memory
instead. A buffer is therefore never in both totals, and EV74 memory and MLA
memory can be added.

On earlier releases without that heap, Sentinel reads `/dev/simaai-mem`, finds
`Total allocated size:`, parses the following hexadecimal byte count, and
converts it to MiB.

This is MLA memory currently allocated. It is not:

- MLA compute utilization;
- model FPS or latency;
- total physical DRAM use;
- a measurement of `MLA 0.68V` power.

If the selected interface cannot be read or its text format is unrecognized,
the value is unavailable rather than zero. Sentinel does not fall back to the
legacy allocator after selecting DMA-BUF telemetry.

## EV74 memory

On Platform 3.0, EV74 (the CVU) works on DMA-BUF memory. While EV74 is using a
buffer, the kernel lists the `cvu` device under that buffer's attached devices
in `/sys/kernel/debug/dma_buf/bufinfo`. Sentinel sums the objects attached to
`cvu` or to a device named `<address>.cvu`, counting each DMA-BUF inode once.
A buffer belongs to the hardware that produced it, so two kinds of object are
left out: those exported by an MLA heap (`mla` or `simaai,dms`), which are MLA
memory, and frames a video decoder or camera ISP produced for EV74 to read.
Sentinel recognises those producers from their device-tree nodes under
`/sys/bus/platform/devices`: a video decoder's `al,ip-type` starts with `dec`,
and a camera ISP's `compatible` includes `arm,isp`.

```text
ev74_mem_allocated_mb = sum of sizes of unique inodes attached to cvu,
                        excluding those exported by an MLA heap
                        and those also attached to a video decoder or camera ISP
```

Each EV74 client attaches to a buffer separately, so one buffer often lists
`cvu` several times; it still counts once. Sentinel reads `bufinfo` once per
sample for both MLA and EV74 memory. The System tab charts this value,
and the Compare Runs tab can overlay `EV74 memory` for selected runs.

Read the value with these limits in mind:

- A shared-pool buffer that EV74 and the MLA share counts as EV74 memory only,
  whichever of them wrote it. In the pipelines tested, these are the
  preprocessed tensors EV74 hands to the model. EV74 memory and MLA memory never
  include the same buffer, so adding them gives the memory both hold.
- Frames a video decoder or camera ISP produced are counted by neither metric,
  even while EV74 reads them. With one RTSP stream that is 12 decoded pictures
  (6.75 MiB at 640x512). The decoder's own working memory is not counted either.
- A buffer counts once EV74 has used it. A tensor allocated for EV74 that EV74
  has not processed yet is `linux,cma` memory, not EV74 memory.
- DMA-BUF names are ignored. System daemons hold small buffers named `EV74`
  that EV74 is not using.
- EV74's fixed firmware reservation and kernel-internal allocations are not
  DMA-BUFs and are not included.
- The CVU driver caches at most 64 buffers per client. A pipeline whose own EV74
  buffers exceed this can have in-use buffers evicted, and the value can read
  low. An application that creates a new input buffer for every frame can make
  it read high: the driver keeps up to 64 old buffers mapped per client, and
  they stay allocated. `/sys/kernel/debug/simaai-cvu/stats` reports evictions.

The value is unavailable rather than zero when `/dev/cvu` is absent or when
`bufinfo` cannot be read or parsed.

## CMA pool

Sentinel also reads `CmaTotal` and `CmaFree` from `/proc/meminfo` on every
normal daemon sample and reports:

```text
CMA used = CmaTotal - CmaFree
```

`CmaTotal` covers every CMA region reserved at boot. `CmaFree` excludes movable
pages, such as page cache, that Linux places in CMA and moves out on demand. CMA
used therefore overstates allocated memory and is not EV74 memory: on an idle
Modalix Platform 3.0 DevKit it read about 90 MiB while EV74 held no memory.
Use `ev74_mem_allocated_mb` for EV74 memory.

The keys `ev74_cma_total_mb`, `ev74_cma_free_mb`, and `ev74_cma_used_mb` keep
their names so existing clients and recorded runs still read them; their labels
are `CMA reserved`, `CMA free`, and `CMA used`. Checkpoint recordings retain
all three. Linux CMA is a shared kernel facility, and Sentinel cannot attribute
these pages to an individual process from `/proc/meminfo` alone. If either
field is absent, the derived used value is reported as unavailable rather than
zero; any field that is present remains available on its own.

The boot reservation can be inspected separately with:

```bash
dmesg | grep -i cma
```
