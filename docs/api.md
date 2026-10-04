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

## Security and concurrency

The socket is local to the DevKit and is never exposed remotely by Sentinel.
It is intentionally accessible to local users because the supported control
operations only start and stop telemetry traces; the API does not delete runs,
execute workloads, or modify hardware. Remote access should be provided by an
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
