# SoM power script provenance

`SoM_Power_Share.py` is the SiMa.ai hardware team's SoM power telemetry
script. It defines how Modalix SoM power is measured: which PMBus registers
are read on the two SoM PMICs (0x4D and 0x4F), how they are decoded, and how
readings are averaged. Sentinel reproduces that method and does not own it.

| | |
| --- | --- |
| Source | SiMa.ai hardware team |
| Added to Sentinel | 2026-11-11 |
| SHA-256 | `be3d3609012c56a2022f0f491e795a1a3eed50260d3faf304208013be0659914` |

The file is stored exactly as received. Do not edit it here: a change to the
method arrives from the hardware team as a new script.

## How Sentinel uses it

`src/power/hw_som_power_share.rs` is a Rust port that keeps the script's
names, constants, and order of operations, and it measures every Modalix SoM
rail. Sentinel replaces only the transport: in-process SMBus transfers instead
of `i2cget`/`i2cset` subprocesses.

`golden.json` holds rail reads replayed through the script itself. `cargo test`
fails if the port returns different values, or if the script no longer matches
the SHA-256 above.

- Cases named `recorded` are raw PMBus words read on a Modalix DevKit
  (Platform 3.0) on 2026-11-11 under idle, CPU, MLA, and combined load.
- Cases named `synthetic` cover failed reads and decode edge cases.

## Updating to a new script

1. Replace `SoM_Power_Share.py` with the new file from the hardware team.
2. Run `python3 hardware/som_power_share/make_golden.py`.
3. Carry the script's changes into `src/power/hw_som_power_share.rs` and set
   `SCRIPT_SHA256` there until `cargo test` passes.
4. Update the table above.
