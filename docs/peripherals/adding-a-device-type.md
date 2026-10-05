# Adding a device type

Sentinel's peripheral catalog lists hardware facts. A new kind of device (for
example a display or a serial adapter) needs a typed record, a provider that
discovers it, tests, and a page in this directory. Whether an application
supports a device is decided by that application, not by Sentinel.

## 1. Define the record

Add a module under `src/peripherals/` with a `serde` struct for the device and
add it as a variant of `Peripheral` in `src/peripherals/mod.rs`. The variant
name becomes the `type` tag in JSON, so `Display(display::Display)` serializes
as `{"type": "display", ...}`. Extend `Peripheral::id`, `kind` and `describe`
for the new variant.

- `id` must be stable across replugs and reboots: derive it from a bus path,
  serial number, or another attribute that survives them, never from an
  enumeration index such as a card number or `/dev/videoN`.
- `id` must also be unique across the whole catalog; the worker does not
  check. Prefix it with the type and provider, as the existing ids do
  (`camera:v4l2:…`, `microphone:alsa:…`), and build the rest from something
  two identical devices cannot share, such as the port they are plugged into.
  A serial number alone is not enough: cheap devices often repeat one.
- Optional facts are `Option` fields marked
  `#[serde(default, skip_serializing_if = "Option::is_none")]`, as in the
  existing records, so an unknown fact is left out of the JSON. A plain
  `Option` would serialize as `null`.
- Report facts the kernel or device exposes. Do not add fields that encode a
  policy, such as whether a mode is supported.

## 2. Write the provider

Implement the `Provider` trait:

```rust
pub trait Provider: Send {
    fn name(&self) -> &'static str;            // e.g. "display.drm"
    fn subsystems(&self) -> &'static [&'static str]; // uevent subsystems, e.g. ["drm"]
    fn discover(&mut self) -> Result<Vec<Peripheral>, ProviderError>;
}
```

`discover` runs one complete scan on the `peripherals` thread. The worker calls
it at startup, after a uevent from one of `subsystems`, and on refresh.

- **Read only.** Open device nodes read-only and use query interfaces only.
  Never acquire, configure, or stream from a device.
- **Bounded.** Cap every list you read and every loop of queries, so a broken
  device cannot stall the scan. The V4L2 helpers in `videodev2.rs` show the
  pattern.
- **Errors.** Return `ProviderError` with a code such as `io.permission_denied`
  or `io.open` (see `sysutil.rs`) when the scan cannot complete. The catalog
  then keeps this provider's last good devices and lists the error in `errors`.
  A device that disappears during the scan is skipped, not an error; the uevent
  that its removal sends triggers a rescan.
- **Degrade, don't fail.** A missing optional attribute leaves its field out.
  Treat an attribute that exists but cannot be read by what it is for:
  - one the scan needs to find or identify a device (its id, its USB
    identity) is an error, so the provider's last good devices are kept;
  - one that only adds a fact still publishes the device without that fact,
    with a reason in the record (the microphone record's `issues` does this
    for unreadable capture metadata).

Register the provider in `builtin_providers()` in `src/peripherals/mod.rs`.

## 3. Test it

Tests read a fixture tree instead of the live system: give the provider its
sysfs, procfs, and `/dev` roots as parameters and fake the ioctl layer, as
`alsa/tests.rs` and `v4l2/tests.rs` do. Cover each way devices of the class
differ, not only the device you have: counts, formats, ranges versus discrete
values, missing optional fields, several identical devices at once, and a
device that disappears mid-scan. Assert that two identical devices get
different ids. Label fixtures copied from real hardware as
real captures.

`cargo test --locked` must pass, and `cargo fmt --check` and `cargo clippy
--locked --all-targets` must report nothing new.

## 4. Document it

Add a page in this directory that lists the provider name, the uevent
subsystems, how `id` is derived, every field with when it is present, the
error codes, and a JSON example. Link it from [the API reference](../api.md)
and [the documentation index](../README.md), then regenerate the translations
(see [localization](../i18n/README.md)).
