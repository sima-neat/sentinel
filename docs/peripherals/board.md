# Board camera configuration

The catalog's `board` block describes how the board is set up for MIPI
cameras: which board it is, which camera overlays U-Boot applies, which camera
sensors the booted device tree describes, and which sensors the installed
overlays can describe. Together with the `camera.mipi` devices it tells a
configured camera that was detected from one that was configured but not
detected. Like the device records, it reports facts only and never changes
the configuration.

The block is read in every scan, after the providers, so `camera_id` always
refers to the `devices` of the same catalog, and `revision` changes when the
block changes. It is absent only until the first scan completes, and the API
returns HTTP 503 as before when discovery is disabled.

## Fields

| Field | Present | Meaning |
| --- | --- | --- |
| `model` | when the device tree has one | `/sys/firmware/devicetree/base/model`, up to its first NUL and without surrounding whitespace, e.g. `SiMa.ai Modalix SoM 16Gig Board` |
| `camera_overlays` | when `fw_printenv -n dtbos` succeeds | The entries of the U-Boot `dtbos` variable that end in `.dtbo`, in their order |
| `configured_cameras` | always | The MIPI CSI-2 sensors on I2C in the live device tree; may be empty |
| `supported_sensors` | always | The sensors the overlay files under `/boot` configure; may be empty |

Each `configured_cameras` entry:

| Field | Meaning |
| --- | --- |
| `compatible` | The node's first `compatible` string, e.g. `sony,imx477` |
| `dt_node` | The node's path from the device-tree root, e.g. `/i2cmux@0/i2c@0/imx477@1a` |
| `i2c_device` | The I2C device, `<bus>-<address>` as sysfs names it, e.g. `5-001a` |
| `data_lanes` | The number of cells in the sensor endpoint's `data-lanes` |
| `driver_bound` | Whether a driver is bound to the I2C device |
| `camera_id` | The `id` of the `camera.mipi` camera that is this sensor; absent when no such camera was detected |

Each `supported_sensors` entry has `compatible` and `overlays`, the sorted
file names of the overlays that configure it. Entries are sorted by
`compatible`.

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
  an I2C sensor's sub-device `<driver> <bus>-<address>`, so the camera whose
  `camera_name` ends with ` <i2c_device>` is this sensor. This matches the
  exact device, without comparing vendor or sensor names.
- **The overlay list** is the platform's own record, kept in the U-Boot
  environment. `fw_printenv` knows where that environment is stored on the
  board, so Sentinel asks it instead of reading flash itself. The value is the
  list U-Boot applies at the next boot; it is the booted list unless it was
  changed after boot.
- **The overlay files** are the overlays the platform ships. Every `*.dtbo`
  file in a directory directly under `/boot` (for example `/boot/boot-0/` and
  `/boot/boot-1/`, the A/B slots) is parsed. A sensor is a node with
  `compatible` and a CSI-2 endpoint, as above, that the overlay adds to an I2C
  bus: below a node whose name starts with `i2c`, or in a fragment whose
  target is one, by `target-path` or by the label in `__fixups__` that its
  `target` refers to. A file name found in several slots is listed once.

A CSI-2 source that has another one below it is a bridge, for example an
I2C mux or a GMSL deserializer and serializer, and is left out in both lists,
so only the sensor itself appears.

Camera resolutions and formats still come from the ISP, in the camera's
`modes`; overlays do not describe them.

## Bounds

Discovery reads at most 1024 I2C devices, 256 nodes of each device's subtree,
4096 entries of `/boot` and of each of its directories, and 512 overlay files
of at most 1 MiB each. Overlay files are parsed once and parsed again only
when their size or modification time changes. `fw_printenv` is killed after
2 seconds.

## Errors

Errors have `provider: "board"`. The rest of the block is still published.

| Code | When |
| --- | --- |
| `io.open` | `model`, `/sys/bus/i2c/devices`, a device's node, `/boot`, one of its directories, or an overlay file exists but cannot be read |
| `io.permission_denied` | The same, failing with `EACCES`; or `fw_printenv` exists but cannot be run |
| `peripherals.discovery_failed` | `fw_printenv` did not finish in time; an overlay file is malformed or larger than 1 MiB; more than 512 overlay files |

Skipped overlay files, and unreadable I2C devices, are reported as one error
each, with the first problem's code and reason and the number of others. A missing `fw_printenv`,
or one that exits unsuccessfully (as it does when `dtbos` is not set), leaves
`camera_overlays` out without an error. A board without a device tree, I2C
devices, or `/boot` reports empty lists without an error.

## Limitations

- The CSI-2 rule is structural. A device with its own CSI-2 endpoint and no
  sensor below it is listed even if it is not a camera, for example a
  HDMI-to-CSI-2 bridge, or a GMSL deserializer whose sensors are not
  described.
- An overlay that adds a sensor to an I2C bus whose label or path does not
  start with `i2c` is not recognised.
- `data_lanes` comes from the first endpoint in node order; a sensor with
  several endpoints of different widths reports one.

## Example

The IMX477 on a Modalix DevKit, as in
[`catalog-example.json`](catalog-example.json). The model, device-tree path,
I2C device and overlay name are transcribed from a DevKit, not captured from
Sentinel; the block is covered by synthetic test fixtures. A board with more
overlays installed lists more of them in `supported_sensors`.

```json
"board": {
  "model": "SiMa.ai Modalix SoM 16Gig Board",
  "camera_overlays": ["modalix-som-waveshare-ARDU-IMX477-1CAM.dtbo"],
  "configured_cameras": [{
    "compatible": "sony,imx477",
    "dt_node": "/i2cmux@0/i2c@0/imx477@1a",
    "i2c_device": "5-001a",
    "data_lanes": 2,
    "driver_bound": true,
    "camera_id": "camera:imx477 5-001a"
  }],
  "supported_sensors": [
    {"compatible": "sony,imx477",
     "overlays": ["modalix-som-waveshare-ARDU-IMX477-1CAM.dtbo"]}
  ]
}
```
