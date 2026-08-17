# Capturing and comparing runs

Sentinel checkpoints persist raw daemon samples around a workload so two or
more application or model runs can be compared on the same elapsed timeline.
The normal live cache remains short-lived under `/run`; completed checkpoints
are stored under `/var/lib/simaai-sentinel/runs` and survive daemon restart and
reboot.

## Capture workflow

```bash
simaai-sentinel checkpoint --name baseline --note "unoptimized model"
./run-test.sh
simaai-sentinel checkpoint --stop

simaai-sentinel checkpoint --name optimized --tag compiler-v2
./run-test.sh
simaai-sentinel checkpoint --stop
```

Names must be unique. Starting while another checkpoint is active, stopping
when none is active, deleting an active run, and selecting an unknown or
ambiguous run all produce actionable errors.

The interactive TUI provides the same basic lifecycle: press `r`, enter a
checkpoint name, and press Enter to start. A red `RECORDING` indicator remains
in the header. Press `r` again to stop and save the run.

An active checkpoint is updated by the daemon using an atomic write and rename
under an exclusive file lock. If the daemon restarts, it resumes appending to
the active checkpoint. Completed files are never rewritten. By default the
newest 50 completed runs are retained; set
`SIMA_SENTINEL_RUN_RETENTION=N` on the daemon and CLI to choose another bound.
An active capture is capped at 21,600 samples (about 12 hours at the default
two-second interval); set `SIMA_SENTINEL_RUN_MAX_SAMPLES=N` to change the
safety limit. Once the limit is reached, new samples are rejected but the run
can still be stopped and saved.

## Run management

```bash
simaai-sentinel runs list
simaai-sentinel runs show baseline
simaai-sentinel runs delete baseline
simaai-sentinel runs clear --force
```

`list` shows recording/completed state, elapsed duration, and sample count.
`show` reports metadata, duration, integrated total-board energy, and metric
count. A run can be selected by its unique name or stable ID.

`clear --force` removes every completed run while preserving an active
recording. The explicit flag prevents accidental bulk deletion.

Each run records:

- name, ID, note, tags, start/end UTC timestamps, and inferred sample interval;
- Sentinel version and available board/OS/SDK identity files;
- metric definitions, units, warning/critical thresholds;
- raw timestamped values, including explicit unavailable values and absent
  keys; and
- enough raw power samples to derive energy.

Energy is calculated in joules by trapezoidal integration of adjacent finite
`power_current_watts` samples. A gap is not interpolated: if either endpoint is
missing, that segment contributes no energy.

## Compare Runs tab

Open Sentinel and select tab `6`:

```bash
simaai-sentinel
```

Controls:

| Key | Action |
| --- | --- |
| `Up` / `Down` | Select a saved run. |
| `Space` | Show or hide it; at most four runs are visible. |
| `Enter` | Designate it as the baseline. |
| `Left` / `Right` | Select the metric series shown for every visible run. |
| `w` | Toggle common-overlap and full-duration windows. |
| `x` | Export the visible selection as versioned JSON under `/tmp`. |
| `d` twice | Delete the selected completed run. |
| `r` | Start or stop a checkpoint. |

Every run begins at elapsed `t=0` at its first captured daemon sample;
wall-clock timestamps and the variable delay between checkpoint creation and
that first sample are not used for alignment. Samples are plotted at their
original relative intervals without resampling or interpolation. The
common-overlap window stops at the shortest selected run. Full duration
preserves longer tails and naturally leaves other series absent.

The visible metric selector provides total board power, maximum board/SoC
temperature, CPU utilization, normalized one-minute CPU load, Linux RAM used,
MLA allocated memory, and EV74 CMA memory used. Only one metric is overlaid at
a time, keeping three or four selected runs readable.

All visible runs use the same Braille line weight and rendering style so no
series appears more prominent than another. Each run still receives a stable
color and distinct legend symbol, and the legend is always visible. The
summary reports sample count, minimum, mean, median, 95th percentile, maximum,
percentage delta from the selected baseline, and integrated energy for power.
In overlap mode, energy is integrated only through the common window.

## CSV export

```bash
simaai-sentinel export baseline optimized \
  --format csv --output comparison.csv
```

CSV uses a long-form schema:

```text
run_id,run_name,elapsed_ms,timestamp,category,metric,unit,value,status
```

There is one row per run, sample, and known metric. `status` is `ok`,
`warning`, `critical`, `unavailable`, or `missing`. CSV fields follow standard
double-quote escaping and load directly in pandas, spreadsheets, and plotting
tools.

## JSON export

```bash
simaai-sentinel export baseline optimized \
  --format json --output comparison.json
```

The JSON document contains export `schema: 1`, generation time, complete run
metadata, metric definitions, raw samples, integrated power energy, and
per-run statistics for every defined metric. Summary `duration_ms` uses the
same first-sample-relative comparison timeline as the TUI and CSV; raw sample
timestamps and checkpoint metadata retain their original UTC values. The
first requested run is the baseline; `baseline_deltas_pct` reports mean
percentage differences for subsequent runs when both means exist and the
baseline is nonzero. Consumers must reject unsupported future schema versions
rather than silently guessing.

Both export formats are written to a temporary sibling and atomically renamed
to the requested output. The output directory must already exist.

## Measurement cautions

- A checkpoint begins when the CLI writes its active record. The first
  captured point is the next daemon cache sample and becomes comparison
  `t=0`; no value is interpolated at the earlier checkpoint-creation time.
- Capture cadence follows the daemon cache interval. Faster PMBus reads are
  already aggregated into the published power fields.
- Missing samples are never filled.
- Partial or failed PMBus passes retain the semantics documented in the
  [Power panel](panels/power.md).
- Comparisons establish measured differences; they do not establish
  statistical significance by themselves.
