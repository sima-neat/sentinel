# Storage and network panel

The Storage/Net panel reports capacity and whole-device I/O for the root eMMC,
optional NVMe storage mounted at `/media/nvme`, aggregate non-loopback network
counters, and default-route uplink utilization for local agents.

## Filesystem capacity

| Field | Source and calculation |
| --- | --- |
| eMMC used / used MB | `statvfs("/")` |
| NVMe used / used MB | `statvfs("/media/nvme")`, only when that path is a mount |

Sentinel calculates:

```text
total = f_blocks × f_frsize
available = f_bavail × f_frsize
used = total - available
used_pct = used / total × 100
```

Because `f_bavail` is space available to an unprivileged process, reserved
filesystem blocks contribute to the displayed used value. `MB` uses binary
MiB conversion.

NVMe metric definitions are added only when `/media/nvme` is detected as a
mount when the daemon starts. Restart Sentinel after mounting or unmounting the
drive so the available metric set matches the system.

## Disk I/O

Sentinel resolves the mount's parent block device from `/proc/mounts`, reads
sector counters from `/proc/diskstats`, multiplies by the device hardware
sector size, and divides the byte delta by elapsed sample time.

The read/write rates therefore describe the whole parent block device, not
only files accessed through the displayed filesystem. Other partitions or
users of the same device can contribute.

If mount-to-device resolution fails, Sentinel uses a device-name heuristic:

- eMMC: first whole `mmcblk*` device;
- NVMe: first whole `nvme*` device.

The first sample has no previous counter baseline and reports zero rate.
Counter resets and device replacement are handled with saturating subtraction,
so they do not create a negative rate.

The UI labels rates as `MB/s`, but calculation divides by 1024 twice and is
therefore MiB/s-equivalent.

## Network fields

| Field | Calculation |
| --- | --- |
| Network RX | Sum of receive-byte deltas from `/proc/net/dev` for every non-loopback interface, divided by elapsed time. |
| Network TX | Sum of transmit-byte deltas from `/proc/net/dev` for every non-loopback interface, divided by elapsed time. |
| Network chart | RX + TX. |
| Uplink RX/TX | Receive/transmit byte delta for the interface selected by the IPv4 default route, divided by elapsed time. |
| Uplink RX/TX utilization | Directional uplink rate in bits per second divided by the interface link speed from `/sys/class/net/<interface>/speed`. |

The aggregation includes physical interfaces, bridges, VLANs, tunnels, and
virtual Ethernet devices when present. The same traffic can therefore be
counted at multiple layers, such as once on a physical interface and again on
a bridge. It is system-interface activity, not necessarily external-wire
throughput.

Interfaces must exist in consecutive samples to contribute a delta. The first
network sample reports zero. The keys end in `_mbps`, but the unit is
MiB/s-equivalent, not megabits per second.

The local API also exposes `network_rx_bytes_per_second`,
`network_tx_bytes_per_second`, `network_rx_utilization_percent`, and
`network_tx_utilization_percent`. These unambiguous metrics only measure the
default-route interface, avoiding the double counting possible in the aggregate
UI counters. Utilization is unavailable when Linux does not report a positive
link speed (for example, for some virtual or wireless interfaces).
