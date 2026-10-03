# External provider protocol (version 1)

An external provider is a program that Sentinel runs to discover devices it
cannot read from the kernel itself. It needs no Sentinel code change or
release. A reference provider and manifest are in
[`examples/peripherals/`](../../examples/peripherals/).

## Manifest

One JSON file per provider in `/usr/lib/simaai-sentinel/providers/`:

```json
{
  "protocol": 1,
  "name": "vendor.lidar.acme",
  "exec": "/usr/lib/simaai-sentinel/providers/acme-lidar",
  "args": ["--list"],
  "subsystems": ["usb", "net"],
  "timeout_ms": 4000,
  "user": "sima"
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `protocol` | yes | Must be `1` |
| `name` | yes | Provider name; every record's `provider` must equal it |
| `exec` | yes | Absolute path of the program |
| `args` | no | Arguments passed to the program |
| `subsystems` | yes | Kernel uevent subsystems that trigger a rescan (`usb`, `sound`, `net`, `video4linux`, ...) |
| `timeout_ms` | no | Time limit per run; default 4000, at most 30000 |
| `user` | yes | Unprivileged user to run as, for example `sima` |

Unknown fields are rejected. The manifest and the program must be owned by
root and not writable by group or others; otherwise Sentinel refuses the
manifest and reports it in the catalog's `issues` as `manifest:<file name without .json>`.

## How Sentinel runs the program

- On every scan: at start-up, after a matching hot-plug event, and on refresh.
- As `user`, with that user's supplementary groups, in its own process group,
  with stdin closed, working directory `/`, and an environment containing only
  `PATH` and `HOME`.
- 10 nice levels below the daemon.
- Killed with its whole process group when it exceeds `timeout_ms`.
- At most 4 MiB of stdout is accepted; the last 8 KiB of stderr is kept for
  error messages.
- In parallel with the other providers. The scan is published when every
  provider has finished or been killed, so keep runs short: a slow provider
  delays the whole update by up to its `timeout_ms`.

## Output

Print exactly one JSON document on stdout, then exit.

Success:

```json
{"schema_version": 1, "ok": true,
 "records": [{"id": "lidar:acme-sn-4411", "type": "lidar",
              "provider": "vendor.lidar.acme", "details": {"model": "A1"}}]}
```

An empty `records` list means no devices, which is normal.

Failure:

```json
{"schema_version": 1, "ok": false,
 "error": {"code": "io.permission_denied", "reason": "cannot open /dev/ttyACM0"}}
```

Records follow the rules in [adding a device type](adding-a-device-type.md#3-the-record).

## Failures and what Sentinel shows

| What happens | `issues[].code` | Effect |
| --- | --- | --- |
| `ok: false` | your `code` | Your last good records stay, marked stale |
| Exceeds `timeout_ms` | `peripherals.provider_timeout` | As above |
| Not valid protocol JSON | `peripherals.invalid_provider_result` | As above |
| Exits non-zero without valid JSON | `peripherals.discovery_failed` (exit status and last stderr line) | As above |
| Program missing or user unknown | `peripherals.provider_unavailable` | As above |
| Manifest refused | `peripherals.provider_rejected` | The provider never runs |

A provider failure never hides another provider's devices.

## Testing

```bash
simaai-sentinel peripherals --test-provider ./acme-lidar.json
```

This runs the program once as you, checks the output exactly as the daemon
does, prints the resulting catalog records, and warns about anything the
installed daemon would refuse (ownership, permissions).
