# Adding a device type

A provider is a Rust module in Sentinel that reads one family of devices from
kernel interfaces (sysfs, `/proc`, uevents, read-only ioctls such as V4L2, ALSA,
IIO or the media controller) and returns records. Sentinel does everything else:
hot-plug wake-ups, failure isolation, last-good records, revisions, the API and
the CLI. Sentinel does not run other programs or load user-space stacks such as
libcamera to discover devices, so first confirm the kernel exposes what
applications need. The camera providers in `src/peripherals/v4l2/` and
`src/peripherals/mipi/` are working examples.

## Rules

1. **Read only.** Open device nodes `O_RDONLY | O_NONBLOCK` and only query them;
   never configure, stream, or take ownership. A running application must never
   notice discovery.
2. **Stable identity.** Build `id` from attributes that survive replugs and
   reboots (bus topology, serial, kernel entity name), never `/dev/videoN` or a
   card number, and prefix it with the type: `microphone:...`.
3. **Cover the class, not your specimen.** List how other devices of the class
   differ (counts, formats, ranges versus discrete values, missing optional
   fields, composite devices, identical devices) and handle each.
4. **Degrade, don't fail.** A missing optional field is omitted. Fail the scan
   only when the provider cannot produce a correct list; Sentinel then keeps your
   last good records and shows the error.
5. **Facts only.** Report what the kernel says. Whether a Neat component supports
   the device belongs in that component's support rules.
6. **Bounded work.** A provider runs inside the daemon and cannot be killed:
   never block, cap enumeration loops at 1024 entries and each device at 4096
   queries per scan (`MAX_ENUMERATION_ENTRIES`, `EnumerationBudget` in
   `src/peripherals/videodev2.rs`).

## Steps

1. Implement `Provider` (`src/peripherals/model.rs`): `name()` (e.g.
   `daemon.audio.alsa`), `subsystems()` (uevent subsystems that trigger a
   rescan, e.g. `sound`), and `discover()` returning records
   `{id, kind, provider, details}`. `kind` is the type token: lowercase letters,
   digits, `_` or `-`; `details` is a JSON object and is published under that key.
2. Register it in `builtin_providers()` in `src/peripherals/mod.rs`.
3. Make filesystem roots injectable and put ioctls behind a small trait so tests
   run without hardware, as the camera providers do. Test each rule above, using
   fixtures transcribed from real captures where you have them.
4. Run it once on a board: `simaai-sentinel peripherals --test-provider
   daemon.audio.alsa` prints the records it would add, with support rules
   applied, and exits non-zero on failure.
5. Document the type from [the template](device-types/TEMPLATE.md) and add it to
   the table in the [overview](README.md).
6. Run the CI steps locally: `cargo fmt --check`, `cargo check --locked`,
   `cargo test --locked`, `scripts/build_vulcan_package.sh`.

Clients read unknown types as JSON, so a new type appears in the API, the CLI,
Insight and Neat Core's `details` without changes to any of them.
