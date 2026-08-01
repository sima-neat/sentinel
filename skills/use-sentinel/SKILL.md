---
name: use-sentinel
description: Read SiMa.ai Sentinel live board telemetry and saved runs, start or stop named telemetry traces, and compare application or model runs through Sentinel's local Unix-socket API. Use for Modalix performance, power, thermal, CPU, RAM, MLA-memory, or EV74 CMA investigations and before/after workload comparisons.
---

# Use Sentinel

Use `scripts/sentinel_api.py` for deterministic access to the local API. Default
to `/run/simaai-sentinel/api.sock`; pass `--socket PATH` only when the user or
test environment supplies another socket.

The API is HTTP/1.1 over a Unix domain socket, not a TCP listener. Choose the
execution path before making requests:

- On the DevKit, run the bundled client directly.
- From an external machine, use an authorized SSH target and execute the
  client on the DevKit. A Unix socket cannot be contacted directly over the
  network.
- Do not expose the socket with `socat` or add a TCP listener. Use SSH unless
  the user explicitly provides an authenticated proxy.

Confirm the SSH target from user input or established session context; do not
scan for DevKits. Verify the daemon and socket before collecting evidence:

```bash
ssh TARGET 'systemctl is-active simaai-sentinel && test -S /run/simaai-sentinel/api.sock'
```

The Sentinel package installs this skill for both Codex and Claude on the
DevKit. Invoke that remote copy so SSH stdin remains available for normal
authentication and command handling:

```bash
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py health'
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py latest'
```

If the remote skill is missing and SSH uses non-interactive authentication,
stream the bundled client with
`ssh TARGET 'python3 - health' < scripts/sentinel_api.py`. Otherwise use local
HTTP-over-socket `curl` through SSH as documented in `references/api.md`.

The socket is normally mode `0666`, so API access should not require `sudo`.
Treat SSH failures and API failures separately when reporting a connection
problem.

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

Run the same lifecycle remotely with the installed client:

```bash
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py active'
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py start --name baseline --note "before optimization" --tag compiler-v1'
# Run workload on the DevKit.
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py stop'
ssh TARGET 'python3 ~/.codex/skills/use-sentinel/scripts/sentinel_api.py runs'
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
