# Sentinel agent API

The API is HTTP/1.1 over `/run/simaai-sentinel/api.sock`, not TCP. Responses are
JSON and include an `error` field on failure.

The socket is local to the DevKit and normally has mode `0666`. An external
agent must execute its API client through an authorized SSH session; it cannot
connect to the Unix socket over the network. When the packaged skill is
installed on the DevKit:

```bash
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py health'
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py start --name test-run --tag manual'
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py stop'
```

For a read-only request without the remote skill, execute `curl` through SSH:

```bash
ssh TARGET 'curl --silent --show-error --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health'
```

Keep the API on the Unix socket; do not create an unauthenticated TCP bridge.

| Client command | API operation |
| --- | --- |
| `health` | `GET /v1/health` |
| `latest` | `GET /v1/samples/latest` |
| `metrics` | `GET /v1/metrics` |
| `active` | `GET /v1/traces/active` |
| `start` | `POST /v1/traces` with `name`, optional `note`, and `tags` |
| `stop` | `POST /v1/traces/stop` |
| `runs` | `GET /v1/runs` |
| `run NAME` | `GET /v1/runs/NAME` |
| `compare A B...` | `GET /v1/compare?runs=A,B,...` |

Trace samples follow the daemon cache cadence, normally two seconds. The API
uses the same locked run store as the CLI and TUI. Compare output contains raw
run metadata, per-run summaries, and percentage deltas relative to the first
run. Add `raw=1` to the compare query only when timestamped samples are needed.

Only one trace can be active. Starting while another trace is active or
stopping when none is active returns a conflict response. Always inspect
`GET /v1/traces/active` before changing trace state.
