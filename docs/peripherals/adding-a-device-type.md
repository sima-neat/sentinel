# Adding a device type

This page is for contributors who want Sentinel to discover a new kind of
device. It covers checking the kernel interface, writing the provider, testing it, and
documenting the new type so application developers can use it.

## 1. Check the kernel describes the device

Every provider is Rust code inside Sentinel that reads kernel interfaces:
sysfs, `/proc`, uevents, and read-only ioctls (V4L2, ALSA, IIO, media
controller, ...). Sentinel does not run other programs or load user-space
stacks such as libcamera or GStreamer to discover devices. Before starting,
confirm the kernel exposes what applications need to know about the device.

The camera providers are working examples: USB cameras in
`src/peripherals/v4l2/` and MIPI cameras in `src/peripherals/mipi/`.

## 2. Rules every provider follows

1. **Read only.** Query the device; never configure it, stream from it, take
   ownership of it, or change kernel state. Open device nodes with
   `O_RDONLY | O_NONBLOCK`. A running application must never notice discovery.
2. **Stable identity.** A record's `id` must stay the same across replugs,
   reboots and renumbering. Build it from stable attributes (bus topology,
   serial, a kernel entity name), never from `/dev/videoN`, a card number or
   enumeration order. Prefix it with the type: `camera:...`, `microphone:...`.
3. **Cover the class, not your specimen.** The device on your desk is the first
   test fixture, not the specification. Before writing code, list how other
   members of the class differ (counts, formats, ranges versus discrete values,
   missing optional fields, composite devices, several identical devices at
   once, values that change while in use) and handle each.
4. **Degrade, don't fail.** A missing optional field omits that field. An
   unreadable optional part says why in the record. Fail the scan only when
   the provider cannot produce a correct list; Sentinel then keeps your last
   good records and shows the error.
5. **Facts only.** Report what the device and kernel say. Whether a Neat
   component supports the device is decided by that component's support rules,
   not by the provider.
6. **Bounded work.** Cap every enumeration loop at 1024 entries and never
   block: a provider runs inside the daemon and cannot be killed.

## 3. The record

A provider returns a list of records:

```json
{"id": "microphone:usb-1-2.3:1.2", "type": "microphone",
 "provider": "daemon.audio.alsa", "details": {"channels": 2, "formats": ["S16_LE"]}}
```

| Field | Rule |
| --- | --- |
| `id` | Non-empty, unique across all providers, stable (see above) |
| `type` | Lowercase letters, digits, `_` or `-`, starting with a letter; at most 64 characters; not `id`, `type` or `provider` |
| `provider` | Your provider's name, for example `daemon.audio.alsa` |
| `details` | A JSON object. Its fields are the type's schema, documented in [device types](device-types/README.md) |

In the catalog, `details` is published under a key named after the type:
`{"id", "type", "provider", "microphone": {...}}`. Clients read unknown types
as JSON, so your type appears in the API, the CLI, Insight and Neat Core's
`details` without changes to any of them.

## 4. Write the provider

1. Create `src/peripherals/<name>/` (or `<name>.rs`) and implement the
   `Provider` trait from `src/peripherals/model.rs`:

   ```rust
   impl Provider for MicrophoneProvider {
       fn name(&self) -> &str { "daemon.audio.alsa" }
       // Kernel uevent subsystems that should trigger a rescan.
       fn subsystems(&self) -> &[String] { &self.subsystems } // ["sound"]
       fn discover(&mut self) -> Result<Vec<Record>, ProviderError> { ... }
   }
   ```

2. Register it: add one line to `builtin_providers()` in
   `src/peripherals/mod.rs`.
3. Make the filesystem roots injectable (for example `with_roots(sys, dev)`)
   and put ioctl calls behind a small trait, so tests run without hardware.
   The camera providers show the pattern.
4. Map failures to `ProviderError` codes: `io.permission_denied`, `io.open`,
   or `peripherals.discovery_failed`.

## 5. Test it

Run your provider once, as Sentinel would, and see exactly what it adds to the
catalog:

```bash
simaai-sentinel peripherals --test-provider daemon.audio.alsa
```

The command validates your provider's records with the same per-provider
checks as the daemon, applies any support rules, prints the result, and exits
non-zero on failure. Ids must also be unique across providers: prefix them with
your type and a provider-specific key.

Unit tests are required:

- one test per variation axis you listed in rule 3;
- fixtures transcribed from real device captures where you have them, and
  format-faithful synthetic fixtures otherwise, labelled as such;
- the error paths: permission denied, a device that disappears mid-scan, a
  malformed answer.

Before a pull request, run the repository's CI steps locally:
`cargo fmt --check`, `cargo check --locked`, `cargo test --locked`, and
`scripts/build_vulcan_package.sh`.

## 6. Document the type

Add `docs/peripherals/device-types/<type>.md` from the
[template](device-types/TEMPLATE.md) and list it in the
[device type index](device-types/README.md). Application developers rely on
this page to read your records. State which behaviour was verified on real
hardware and which only on fixtures.

## Checklist

- [ ] The kernel exposes what applications need about the device
- [ ] Read-only; no configuration, streaming or ownership
- [ ] Stable `id` from stable attributes, prefixed with the type
- [ ] Variation axes listed and each one tested
- [ ] Optional data degrades instead of failing
- [ ] `--test-provider` output reviewed
- [ ] Device type page added and indexed
- [ ] Repository CI steps pass locally
