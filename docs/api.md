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
| `GET /v1/health` | Version, freshness, sample and metric counts, errors, and active trace. |
| `GET /v1/cache` | Complete live cache document. |
| `GET /v1/metrics` | Metric definitions, units, descriptions, and thresholds. |
| `GET /v1/samples/latest` | Latest timestamped metric values. |
| `GET /v1/traces/active` | Active trace or `null`. |
| `POST /v1/traces` | Start a named trace. |
| `POST /v1/traces/stop` | Stop and persist the active trace. |
| `POST /v1/traces/{id}/stop` | Stop the active trace only if its ID still matches. |
| `GET /v1/runs` | List active and completed run summaries. |
| `GET /v1/runs/{name-or-id}` | Retrieve a saved run and raw samples. |
| `GET /v1/compare?runs=A,B` | Compare two or more runs; the first is the baseline. |
| `GET /v1/peripherals` | Connected peripherals, from memory. |
| `POST /v1/peripherals/refresh` | Rescan, then return the new catalog. |

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

Clients that first read the active trace should include its ID when stopping it:

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/20261004T120000.000Z-baseline/stop
```

The conditional form returns HTTP 409 without stopping anything if another
client has replaced the trace. The ID comparison and stop occur under the same
run-store lock.

Names must be unique, and only one trace may be active. Conflicting lifecycle
operations return HTTP 409. Unknown runs and routes return HTTP 404. Invalid
requests return HTTP 400. Responses use JSON `null` for unavailable metrics.
Comparison responses contain run metadata, statistics, and baseline deltas by
default. Add `raw=1` only when timestamped samples are required.

## Peripherals

A discovery thread scans the board's peripherals when the daemon starts, when
the kernel reports a device change, and on refresh requests. Device providers only
read kernel interfaces and never open a stream. To read the U-Boot overlay list
for the `board` block, each scan also runs `fw_printenv -n dtbos` as root. It
never changes the environment, but it takes the environment's lock, which
creates `/var/lock/fw_printenv.lock`. `GET /v1/peripherals` returns the latest
result:

```json
{"revision": 1791155282460, "observed_at": "2026-10-04T23:08:02.460Z",
 "board": {"model": "SiMa.ai Modalix SoM 16Gig Board", "configured_cameras": [...], ...},
 "devices": [{"type": "camera", "id": "camera:v4l2:3f2a9c0d41b7e650", ...}],
 "errors": [{"provider": "camera.v4l2", "code": "io.permission_denied", "reason": "..."}]}
```

| Field | Meaning |
| --- | --- |
| `revision` | Changes whenever `board`, `devices` or `errors` change. It starts from a random value when the daemon starts, so a restart or a clock change is very unlikely to repeat one; compare it for equality only. It stays below 2^52, so it is exact in JSON readers that use doubles. |
| `observed_at` | When the scan behind this result started; `null` until the first scan completes. |
| `board` | How the board is set up for cameras: model, the overlays U-Boot applies, the cameras the device tree configures (each with the `id` of the `camera.mipi` camera that is its sensor, when the catalog has one), and the sensors the installed overlays support. Read in the same scan as `devices`; absent until the first scan completes. See [board camera configuration](peripherals/board.md). |
| `devices` | One object per device, tagged by `type`. `id` is stable across replugs into the same port and is never a `/dev/videoN` name. |
| `errors` | Providers that failed in the latest scan, and fields of the `board` block that could not be read completely (provider `board.<field>`, for example `board.overlays`). A failed provider's devices from its last successful scan stay in `devices`, and a failed `fw_printenv` keeps the last overlay list. `hotplug.unavailable` means kernel uevents cannot be received, so rescans happen only on refresh. |

Sentinel reports hardware facts only. Whether an application supports a
device or mode is decided by that application.

`POST /v1/peripherals/refresh` waits for a scan that starts after the request
and returns HTTP 200 with that catalog, the same document as `GET`. Concurrent
refreshes share scans. After every scan the discovery thread rests for 1
second, so a refresh can wait up to that long before its scan starts, and
repeated refreshes cannot keep discovery running. It returns HTTP 504 if the scan does not finish within
10 seconds, and HTTP 429 when 8 refresh requests are already waiting. Both
routes return HTTP 503 when discovery is disabled (`--no-peripherals`) or has
stopped.

`simaai-sentinel peripherals` prints the same result as a table; add `--json`
for the raw document and `--refresh` to rescan first.

Device records are described per type: [USB cameras](peripherals/camera.md),
[MIPI CSI-2 cameras](peripherals/camera-mipi.md),
[microphones](peripherals/microphone.md). The `board` block is described in
[board camera configuration](peripherals/board.md).

[`peripherals/catalog-example.json`](peripherals/catalog-example.json) is a
complete response from a DevKit, with the USB camera trimmed to one mode per
format. Its `board` block is transcribed from the DevKit rather than
captured. A Sentinel test checks it against the schema, so clients can test
against it.

## Security and concurrency

The socket is local to the DevKit and is never exposed remotely by Sentinel.
It is intentionally accessible to local users, so clients such as Insight and
the CLI work without root. Its control operations start and stop telemetry
traces and request a peripheral rescan; the API does not delete runs, execute
workloads, or modify hardware. A rescan only reads, and the 1-second rest after
every scan bounds how much work refresh requests can cause. A local user who
keeps 8 refreshes waiting makes other refreshes return HTTP 429, while `GET`
keeps working. Remote access should be provided by an
authenticated Kerrigan/Fleet Manager proxy, not by forwarding this socket or
adding an unauthenticated TCP listener.

Cache writes use atomic rename. Run operations use the same exclusive file
lock as the CLI, TUI, and daemon recorder. Use the ID-bearing stop route when a
client must not stop a trace that replaced the one it observed.

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
