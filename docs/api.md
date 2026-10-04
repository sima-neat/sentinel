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

`GET /v1/peripherals` returns the catalog of connected devices that the
peripherals thread writes to `/run/simaai-sentinel/peripherals.json`, exactly as
written. How discovery works, the record format of each device type, and how
to add one are in [Peripherals](peripherals/README.md).

| Field | Meaning |
| --- | --- |
| `instance_id` | New on every daemon start. A different value means the daemon restarted. |
| `state`, `ready` | `starting` until the first scan, then `ready`, or `degraded` while a provider, the event monitor, or the support rules report a problem. A stopped daemon leaves `degraded` with error `peripherals.stopped`. |
| `revision` | Increases whenever anything a client can see changes: devices, issues, errors, or the support rules status. |
| `scan_sequence` | Increases after every completed scan, including unchanged ones. |
| `stale`, `issues`, `error` | A provider that failed keeps its last good records, marked by `retained_last_good`; other providers are unaffected. `error` describes the daemon itself, such as hot-plug monitoring being unavailable. |
| `changes` | The last 256 changes: `added`, `removed`, `changed`, `error`, `recovered`, each with `sequence` and `revision`. |
| `support` | Which Neat Core rules classified the camera modes; see [support rules](peripherals/README.md#support-rules). |
| `devices` | `{id, type, provider, <type>: {...}}`. The `id` survives replugs into the same port and is never a `/dev/videoN` name. |

To poll cheaply, send the last `revision` and `instance_id` you saw:
`GET /v1/peripherals?since_revision=7&instance_id=<id>`. If nothing a client
can see has changed, the reply is
`{"unchanged": true, "revision": 7, "scan_sequence": ..., "instance_id": ...}`;
otherwise it is the full catalog. `since_revision` without `instance_id` is
rejected with HTTP 400, because revisions restart with every daemon.

`POST /v1/peripherals/refresh` schedules a scan and returns
`{"accepted": true, "target_scan_sequence": N, "instance_id": ...}`; the
refresh is complete when the catalog with that `instance_id` reaches
`scan_sequence` `N`. The daemon accepts every request but starts explicit
refresh scans no more than once every five seconds; requests within that
cooldown share the next scan.
HTTP 503 means peripheral discovery is not running or has stopped, or the
catalog file could not be written (for example, `/run` is full) and is out of
date; the `error` says which, and the journal has details. In these cases
`/v1/health` reports `"peripherals": null`. Sentinel retries a failed write
every second and serves the catalog again once one succeeds. HTTP 503 from
`GET /v1/peripherals` can also mean the catalog cannot be read.

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
