# Local agent API

The Sentinel daemon exposes a versioned HTTP/JSON API over the local Unix
socket `/run/simaai-sentinel/api.sock`. It does not listen on a TCP port. The
API and terminal UI use the same cache and locked checkpoint store, so traces
started by an agent are immediately visible in the UI and CLI.

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## Endpoints

| Method and path | Purpose |
| --- | --- |
| `GET /v1/health` | Version, freshness, sample and metric counts, errors, active trace, and a `peripherals` summary. |
| `GET /v1/cache` | Complete live cache document. |
| `GET /v1/metrics` | Metric definitions, units, descriptions, and thresholds. |
| `GET /v1/samples/latest` | Latest timestamped metric values. |
| `GET /v1/traces/active` | Active trace or `null`. |
| `POST /v1/traces` | Start a named trace. |
| `POST /v1/traces/stop` | Stop and persist the active trace. |
| `GET /v1/runs` | List active and completed run summaries. |
| `GET /v1/runs/{name-or-id}` | Retrieve a saved run and raw samples. |
| `GET /v1/compare?runs=A,B` | Compare two or more runs; the first is the baseline. |
| `GET /v1/peripherals` | Current peripheral catalog. Add `since_revision=N` to get a short `unchanged` reply when nothing changed. |
| `POST /v1/peripherals/refresh` | Request a rescan; returns `target_scan_sequence`. |

Start request example:

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -H 'Content-Type: application/json' \
  -d '{"name":"baseline","note":"before optimization","tags":["compiler-v1"]}' \
  http://localhost/v1/traces
```

Stop the active trace:

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/stop
```

Names must be unique, and only one trace may be active. Conflicting lifecycle
operations return HTTP 409. Unknown runs and routes return HTTP 404. Invalid
requests return HTTP 400. Responses use JSON `null` for unavailable metrics.
Comparison responses contain run metadata, statistics, and baseline deltas by
default. Add `raw=1` only when timestamped samples are required.

## Peripheral catalog

Sentinel keeps a catalog of connected peripherals, currently cameras. A
dedicated daemon thread waits for kernel hot-plug events (uevents), waits
250 ms for the burst to settle, runs every discovery provider once, and
replaces `/run/simaai-sentinel/peripherals.json` by atomic rename. It does not
scan on a timer. Providers only query devices: they never open a stream,
change a format, or take ownership of a camera.

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
```

Catalog fields:

| Field | Meaning |
| --- | --- |
| `instance_id` | New on every daemon start. A different value means the daemon restarted. |
| `state`, `ready` | `starting` until the first scan, then `ready`, or `degraded` while a provider or the event monitor reports a problem. |
| `revision` | Increases only when the device list or a device's details change. |
| `scan_sequence` | Increases after every completed scan, including unchanged ones. |
| `stale`, `issues` | A provider that failed keeps its last good records, marked by `retained_last_good`; other providers are unaffected. |
| `changes` | The last 256 changes: `added`, `removed`, `changed`, `error`, `recovered`, each with `sequence` and `revision`. |
| `devices` | `{id, type, provider, <type>: {...}}`. The `id` is stable across replugs and never a `/dev/videoN` name. |

Each camera mode carries `supported` and `reason`: whether the installed Neat
Core's `CameraInput` accepts it. Sentinel does not decide this itself. Neat
Core installs its rules at `/usr/share/simaai-sentinel/support/neat-core.json`,
and the peripherals thread applies them to every mode before the catalog is
compared and written, so a Core upgrade bumps `revision` like any other change.
Sentinel watches that directory and re-applies the rules without rescanning
hardware. The top-level `support` field reports `state` (`applied`,
`not_installed`, `invalid`, or `stale` when an invalid update left the
previous rules in use), the rules' `source`, and their `path`. Without Neat
Core, every mode is `supported: false` with a reason saying Core is not
installed.

```json
{
  "format": 1,
  "source": "neat-core 0.4.0",
  "camera": {
    "backends": {"accept": ["mipi"], "reason": "..."},
    "formats": {"accept": ["NV12"], "reason": "..."},
    "framerates": {"accept": [{"num": 30, "den": 1}], "reason": "..."},
    "isp_output": {"reason": "..."}
  }
}
```

Rules are checked in that order and the first failure becomes the mode's
`reason`. Size ranges are never marked supported; `isp_output`, when present,
requires the mode to be an ISP output size.

Sentinel creates `/usr/share/simaai-sentinel/support/` but never installs a
file in it: each file there belongs to the package that provides it, so
Sentinel and Neat Core never claim the same path and can be installed,
upgraded, or removed independently. Sentinel keeps reading every rules
format it has supported; a newer format makes Sentinel keep its previous rules
and report that Sentinel needs an update.

Discovery runs at nice +10, so a scan yields to camera pipelines on a busy
board.

Clients should poll with `since_revision` rather than re-read the full
document. After `POST /v1/peripherals/refresh`, re-read until `scan_sequence`
reaches the returned `target_scan_sequence`. A missing or unreadable catalog
returns HTTP 503.

Cameras are discovered from kernel interfaces only. The record format of each
device type, the external provider protocol, and how to add a device type are
in [Peripherals](peripherals/README.md).

## Security and concurrency

The socket is local to the DevKit and is never exposed remotely by Sentinel.
It is intentionally accessible to local users because the supported control
operations only start and stop telemetry traces; the API does not delete runs,
execute workloads, or modify hardware. Remote access should be provided by an
authenticated Kerrigan/Fleet Manager proxy, not by forwarding this socket or
adding an unauthenticated TCP listener.

Cache writes use atomic rename. Run operations use the same exclusive file
lock as the CLI, TUI, and daemon recorder. A trace started through the API is
therefore safe to inspect or stop through any supported interface.

## Agent skill

During installation, the generated Sentinel install script runs `sima-cli
playbooks install` using the exact Sentinel Git commit from which the package
was built. This keeps the runtime and skill revisions aligned without adding
nested skill resources to the Vulcan artifact. The playbook manager installs
the skill for each supported agent and records its source commit in the local
registry.

Sentinel installation on a DevKit always starts with `sima-cli neat install
sentinel`, so no separate skill-install step is normally required there. To
use the Sentinel skill in an environment where the DevKit package is not
installed—for example, an SDK container or developer workstation—install the
skill directly from GitHub with that environment's `sima-cli`:

```bash
sima-cli playbooks install gh:sima-neat/sentinel/skills/use-sentinel
```

To test a revision before it reaches `main`, append the Git ref:

```bash
sima-cli playbooks install --force \
  gh:sima-neat/sentinel/skills/use-sentinel@feature/checkpoint-run-comparison
```

Updating or removing the skill remains under `sima-cli playbooks` management.
