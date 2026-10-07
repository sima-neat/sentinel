# Board camera configuration

The catalog's `board` block describes how the board is set up for MIPI
cameras: which board it is, which overlays U-Boot applies, which camera
sensors the booted device tree describes, and which sensors the installed
overlays can describe. Each configured camera names the `camera.mipi` camera
that is its sensor, so a client can find a camera's `compatible` and, in
`supported_sensors`, the overlays that configure it. Like the device records,
it reports facts only and never changes the configuration.

The block is read in every scan, after the providers, so `camera_id` always
refers to the `devices` of the same catalog, and `revision` changes when the
block changes. It is absent only until the first scan completes, and the API
returns HTTP 503 as before when discovery is disabled.

## Fields

| Field | Present | Meaning |
| --- | --- | --- |
| `model` | when the device tree has one | `/sys/firmware/devicetree/base/model`, up to its first NUL and without surrounding whitespace, e.g. `SiMa.ai Modalix SoM 16Gig Board` |
| `overlays` | once `fw_printenv -n dtbos` has succeeded | The entries of the U-Boot `dtbos` variable that end in `.dtbo`, in their order: every overlay U-Boot applies, not only camera overlays (PCIe, secure-boot and flash overlays use the same variable). Sentinel does not say which entry configures a camera; `supported_sensors` lists the overlays that configure a sensor. When a later run fails, the list of the last successful run is kept and the failure is reported |
| `configured_cameras` | always | The MIPI CSI-2 sensors on I2C in the live device tree; may be empty |
| `supported_sensors` | always | The sensors the overlay files under `/boot` configure; may be empty |

Each `configured_cameras` entry:

| Field | Meaning |
| --- | --- |
| `compatible` | The node's first `compatible` string, e.g. `sony,imx477` |
| `dt_node` | The node's path from the device-tree root, e.g. `/i2cmux@0/i2c@0/imx477@1a` |
| `i2c_device` | The I2C device, `<bus>-<address>` as sysfs names it, e.g. `5-001a` |
| `data_lanes` | The number of cells in the sensor endpoint's `data-lanes` |
| `camera_id` | The `id` of the `camera.mipi` camera that is this sensor; absent when the catalog has no such camera |

Each `supported_sensors` entry has `compatible` and `overlays`, the sorted
file names of the overlays that configure it. Entries are sorted by
`compatible`. The list merges `/boot` and every slot directory under it, so it
can include overlays that are installed only in a slot that is not booted.

## Where each fact comes from

- **The live device tree** is what the kernel booted with, after U-Boot
  applied the overlays. It is the configuration in effect, so the model and
  the configured cameras come from it rather than from the overlay files.
- **The I2C `of_node` link** is how a configured camera is found and matched.
  Every I2C client in `/sys/bus/i2c/devices` (named `<bus>-<address>`;
  adapters, named `i2c-N`, are skipped) whose `of_node` link resolves into
  the live device tree is a candidate. It is a configured camera when its node
  has `compatible` and an `endpoint` node below it, not inside another
  device's node, has `data-lanes`, which marks a MIPI CSI-2 source. V4L2 names
  an I2C sensor's sub-device `<driver> <bus>-<address>`, sometimes followed by
  another word (`ccs 5-0010 pixel_array`), so the camera whose `camera_name`
  has `<i2c_device>` as a whole space-separated word is this sensor. This
  matches the exact device, without comparing vendor or sensor names.
- **The overlay list** is the platform's own record, kept in the U-Boot
  environment. `fw_printenv` knows where that environment is stored on the
  board, so Sentinel asks it instead of reading flash itself. The value is the
  list U-Boot applies at the next boot; it is the booted list unless it was
  changed after boot. This is the only part of discovery that runs a program
  instead of reading kernel interfaces. `fw_printenv` runs as root and never
  changes the environment, but it takes the environment's lock: libubootenv,
  which the DevKit ships, creates `/var/lock/fw_printenv.lock` and waits for
  an exclusive `flock` on it, so a `fw_setenv` running at the same time can
  hold a scan until the timeout.
- **The overlay files** are the overlays the platform ships. Every `*.dtbo`
  file directly in `/boot`, where Platform 3.0 installs them, or in a directory
  directly under it, as in the A/B slots `/boot/boot-0/` and `/boot/boot-1/` of
  Platform 2.1, is parsed. A sensor is a node with
  `compatible` and a CSI-2 endpoint, as above, that the overlay adds to an I2C
  bus: below a node whose name starts with `i2c`, or in a fragment whose
  target is one, by `target-path` or by the label in `__fixups__` that its
  `target` refers to. A file name found in several of these directories is
  listed once.

A CSI-2 source that has another one below it is a bridge, for example an
I2C mux or a GMSL deserializer and serializer, and is left out in both lists,
so only the sensor itself appears.

Camera resolutions and formats still come from the ISP, in the camera's
`modes`; overlays do not describe them.

## Bounds

Discovery reads at most 1024 I2C devices; of each device's subtree, 256
nodes, 256 entries of each node, and 32 levels of nodes counting the device's
own; 4096 entries of `/boot` and of each of its directories; and 512 overlay
files of at most 1 MiB each. A list cut at one of these bounds is reported as
an error of its field, and what was read is still published. Overlay files are
parsed once and parsed again only when their size, modification time, change
time or inode changes; a file that could not be read is read again in the next
scan. `fw_printenv` is killed after 2 seconds. A scan waits at most another
0.5 seconds for it to exit, and then leaves it to be reaped in the background.
At most 64 KiB of its output and 4 KiB of its error output are read.

## Errors

Each error names the field it leaves incomplete: `provider` is
`board.model`, `board.overlays`, `board.configured_cameras` or
`board.supported_sensors`. The rest of the block is still published.

| Code | When |
| --- | --- |
| `io.open` | `model`, `/sys/bus/i2c/devices`, a device's node, `/boot`, one of its directories, or an overlay file exists but cannot be read |
| `io.permission_denied` | The same, failing with `EACCES`; or `fw_printenv` exists but cannot be run (`EACCES` or `EPERM`) |
| `peripherals.discovery_failed` | `fw_printenv` cannot be started for another reason, its output cannot be read, waiting for it fails, it does not finish in time, or it exits unsuccessfully for a reason other than `dtbos` not being set (the reason names the first line it printed to stderr); an overlay file is malformed or larger than 1 MiB; a list is cut at one of the bounds above |

The problems of each list are reported as one error, with the first
problem's code and reason and the number of others. For `supported_sensors`,
an unreadable or cut listing of `/boot` or one of its directories counts with
the skipped overlay files; for `configured_cameras`, a cut listing of I2C
devices counts with the devices that could not be read or were cut. A missing
`fw_printenv`, or one that exits unsuccessfully saying the variable is
`not defined` (as u-boot-tools does when `dtbos` is not set), leaves `overlays`
out without an error. libubootenv, which the DevKit ships, prints an empty value
for a variable that is not set, so `overlays` is then an empty list. Any other
failure keeps the last list and reports an error; before any run has
succeeded, `overlays` is left out. A board without a device
tree, I2C devices, or `/boot` reports empty lists without an error.

## Limitations

- The CSI-2 rule is structural. A device with its own CSI-2 endpoint and no
  sensor below it is listed even if it is not a camera, for example a
  HDMI-to-CSI-2 bridge, or a GMSL deserializer whose sensors are not
  described.
- An overlay that adds a sensor to an I2C bus whose label or path does not
  start with `i2c` is not recognised.
- `data_lanes` comes from the first endpoint in node order; a sensor with
  several endpoints of different widths reports one.
- A sensor whose endpoint has no `data-lanes` is not recognised as a CSI-2
  source, so it is missing from `configured_cameras` and `supported_sensors`.
- A companion chip with its own CSI-2 endpoint is listed as a configured
  camera, for example the `Metoak,xc9080` in the METOAK-DUAL overlay. No
  `camera.mipi` camera is its sensor, so it never has a `camera_id`.
- Nothing watches `/boot` or the U-Boot environment. A change made with
  `fw_setenv`, or an overlay file installed or removed, appears in the next
  scan, which runs after a camera or sound device event or on a refresh.

## Example

The IMX477 on a Modalix DevKit, as in
[`catalog-example.json`](catalog-example.json). The model, device-tree path,
I2C device and overlay name are transcribed from a DevKit, not captured from
Sentinel; the block is covered by synthetic test fixtures. A board with more
overlays installed lists more of them in `supported_sensors`.

```json
"board": {
  "model": "SiMa.ai Modalix SoM 16Gig Board",
  "overlays": ["modalix-som-waveshare-ARDU-IMX477-1CAM.dtbo"],
  "configured_cameras": [{
    "compatible": "sony,imx477",
    "dt_node": "/i2cmux@0/i2c@0/imx477@1a",
    "i2c_device": "5-001a",
    "data_lanes": 2,
    "camera_id": "camera:imx477 5-001a"
  }],
  "supported_sensors": [
    {"compatible": "sony,imx477",
     "overlays": ["modalix-som-waveshare-ARDU-IMX477-1CAM.dtbo"]}
  ]
}
```
