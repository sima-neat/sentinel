---
name: use-sentinel
description: Read SiMa.ai Sentinel live board telemetry and saved runs, start or stop named telemetry traces, and compare application or model runs through Sentinel's local Unix-socket API. Use for Modalix performance, power, thermal, CPU, RAM, MLA-memory, or EV74 CMA investigations and before/after workload comparisons.
---

# Use Sentinel

Use `scripts/sentinel_api.py` for deterministic access to the local API. Default
to `/run/simaai-sentinel/api.sock`; pass `--socket PATH` only when the user or
test environment supplies another socket.

## Inspect

1. Run `health` before collecting evidence.
2. Run `latest` for the current sample or `metrics` to discover keys and units.
3. Treat JSON `null` as unavailable, never as zero.
4. Report timestamps and metric units with conclusions.

```bash
python3 scripts/sentinel_api.py health
python3 scripts/sentinel_api.py latest
python3 scripts/sentinel_api.py metrics
```

## Trace a workload

1. Check `active`; do not replace an unrelated active trace.
2. Start a uniquely named trace immediately before the workload.
3. Execute or ask the user to execute the workload.
4. Stop the trace even when the workload fails, preserving partial evidence.
5. List runs and inspect or compare the completed trace.

```bash
python3 scripts/sentinel_api.py active
python3 scripts/sentinel_api.py start --name baseline --note "before optimization" --tag compiler-v1
# Run workload.
python3 scripts/sentinel_api.py stop
python3 scripts/sentinel_api.py runs
```

Starting and stopping traces changes Sentinel state. Confirm the requested name,
note, and workload boundary before starting. Never stop a trace owned by another
test without user authorization.

## Compare

Pass the baseline first because Sentinel calculates deltas relative to the first
run. Prefer stable run names or full IDs when names are ambiguous.

```bash
python3 scripts/sentinel_api.py compare baseline optimized
python3 scripts/sentinel_api.py run optimized
```

Add `--raw` to `compare` only when timestamped samples are required; the
default compact response is better for agent context.

Summarize duration, samples, energy, mean, maximum, p95, and baseline delta for
the metrics relevant to the request. Note missing samples rather than filling
gaps. Read `references/api.md` only when endpoint or response details are needed.
