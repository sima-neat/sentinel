# Sentinel agent API

The API is HTTP/1.1 over `/run/simaai-sentinel/api.sock`, not TCP. Responses are
JSON and include an `error` field on failure.

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
