# Peripherals

Sentinel keeps a catalog of the devices connected to a Modalix DevKit: which
devices are present, how to identify them, and what they can do. Neat Core,
Insight, sima-cli2, scripts and agents all read the same catalog, so a device
type added here becomes visible everywhere at once.

The catalog is built for growth. Cameras are the first device type; adding
another type (a microphone, an IMU, a LiDAR, ...) does not require changes to
the catalog, the API, the CLI, or the clients.

## How it works

```text
kernel hot-plug event ─┐
refresh request ───────┤
                       ▼
            peripherals thread (one per daemon, sleeps until woken)
              1. run every provider (read-only)
              2. apply Neat Core's support rules (cameras)
              3. compare with the last catalog; revision +1 if changed
              4. write /run/simaai-sentinel/peripherals.json
                       │
                       ▼
            GET /v1/peripherals  ·  simaai-sentinel peripherals
```

A **provider** is Rust code inside Sentinel that discovers one family of
devices from kernel interfaces and returns records. Sentinel does everything
else: waking on hot-plug, debouncing, isolating failures,
keeping a failed provider's last good records, stable revisions, the change
log, the API and the CLI.

Discovery is cheap by design: the thread uses no CPU while nothing changes,
runs 10 nice levels below the daemon, and merges bursts of events and refresh
requests into one scan.

## Pages

| Page | For |
| --- | --- |
| [Adding a device type](adding-a-device-type.md) | Contributors adding support for a new kind of device |
| [Device types](device-types/README.md) | The record format of each supported type, starting with [cameras](device-types/camera.md) |
| [Local agent API](../api.md) | `GET /v1/peripherals`, `POST /v1/peripherals/refresh`, polling with `since_revision` |

## Using the catalog

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

From a Neat application, `simaai::neat::peripherals::list()` (C++) and
`pyneat.peripherals.list()` (Python) return the same catalog. Every device
carries its details as JSON (`details_json` / `details`), so a new device type
is usable from Neat applications before Core adds typed fields for it.
