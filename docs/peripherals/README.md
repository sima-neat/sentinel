# Peripherals

Sentinel keeps a catalog of the devices connected to a Modalix DevKit: which
are present, how to identify them, and what they can do. Neat Core, Insight,
scripts and agents read the same catalog over the
[local agent API](../api.md#peripheral-catalog), so a device type added here
appears everywhere at once.

```bash
simaai-sentinel peripherals          # table
simaai-sentinel peripherals --json   # the catalog document
```

## How it works

A dedicated daemon thread sleeps until a kernel hot-plug event or a refresh
request, waits 250 ms for the burst to settle, runs every provider once, applies
Neat Core's support rules, and replaces `/run/simaai-sentinel/peripherals.json`
when anything changed. It never scans on a timer, uses no CPU while idle, and
runs 10 nice levels below the daemon. Providers only query devices; they never
open a stream, change a format, or take ownership of a device.

| Device type | Provider | Page |
| --- | --- | --- |
| `camera` | `daemon.camera.mipi`, `daemon.camera.v4l2` | [Cameras](device-types/camera.md) |

To support a new kind of device, see [Adding a device type](adding-a-device-type.md).

## Support rules

Each camera mode carries `supported` and `reason`: whether the installed Neat
Core's `CameraInput` accepts it. Neat Core installs these rules at
`/usr/share/simaai-sentinel/support/neat-core.json`; the peripherals thread
applies them before the catalog is compared and written, and re-applies them
without rescanning when the file changes. Without Neat Core, every mode is
unsupported with the reason "Neat Core is not installed".

```json
{"format": 1, "source": "neat-core 0.4.0",
 "camera": {"backends": {"accept": ["mipi"], "reason": "..."},
            "formats": {"accept": ["NV12"], "reason": "..."},
            "framerates": {"accept": [{"num": 30, "den": 1}], "reason": "..."},
            "isp_output": {"reason": "..."}}}
```

Rules are checked in that order and the first failure is the mode's `reason`;
size ranges are never supported. The catalog's `support` field reports `state`
(`applied`, `not_installed`, `invalid`, or `stale` when an invalid update left
the previous rules in use), `source` and `path`. Sentinel creates the directory
but never installs a file in it, so the two packages never claim the same path.
A rules file in a newer format than Sentinel reads keeps the previous rules and
reports that Sentinel needs an update.

## Daemon options

| Option | Default |
| --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` |
| `--no-peripherals` | discovery on |
