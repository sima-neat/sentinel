# USB cameras

USB Video Class (UVC) cameras, discovered through V4L2 by the `camera.v4l2`
provider. A rescan follows every `video4linux` uevent.

The provider walks `/sys/class/video4linux` and keeps only nodes with a USB
ancestor, so platform and ISP nodes are never opened. Each candidate is opened
`O_RDONLY | O_NONBLOCK` and receives query ioctls only (`VIDIOC_QUERYCAP`,
`VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES`, `VIDIOC_ENUM_FRAMEINTERVALS`).
Metadata-only, output-only and memory-to-memory nodes are dropped after
`VIDIOC_QUERYCAP`, so a UVC camera with a metadata node appears once.

## Identity

`id` is `camera:v4l2:<16 hex digits>`, the FNV-1a hash of `identity.stable_key`
(`sysfs:<USB topology>:interface=<bInterfaceNumber>:index=<node index>`). It is
unchanged across replugs into the same port and `/dev/videoN` renumbering. A
different port is a different camera, and identical cameras in different ports
have different ids.

`identity.camera_name`, present for nodes of the `uvcvideo` driver, is the name
Neat's `UsbCameraInput` opens the camera by (`UsbCameraInputOptions::camera_name`),
built as Neat builds it after libcamera: the firmware node of the USB device or
its nearest ancestor with a firmware node (its device-tree path below
`/sys/firmware/devicetree`, else its ACPI `firmware_node/path`), the interface
name without its bus number, and `idVendor:idProduct`, for example
`/base/pci@7000000000-3.1:1.0-1a2b:3c4d`. It is omitted when a part cannot be
read.

## Fields

| Field | Present | Meaning |
| --- | --- | --- |
| `type` | always | `camera` |
| `id` | always | See above |
| `backend` | always | `v4l2` (a USB camera) |
| `model` | when known | The USB `product` string, else the driver's card name |
| `device_path` | always | `/dev/videoN`; routing only, not identity |
| `by_id_path` | with a udev link | The `/dev/v4l/by-id/...` link that resolves to `device_path` |
| `identity` | always | `stable_key`, `topology`, `interface`, `node_index`, and, when the USB device reports them, `vendor_id`, `product_id`, `serial`, `manufacturer` and `speed` (sysfs speed in Mb/s as the kernel prints it, e.g. `"480"`); `driver`, the kernel driver bound to the node's USB interface (e.g. `uvcvideo`), when one is; `camera_name` (see above) |
| `availability` | always | `{"state": "unknown", "reason": ...}`: discovery never opens a stream, so it cannot tell whether the camera is in use |
| `modes` | always | What the camera can output; see below |

## Modes

One mode per format and frame size, sorted by format and then size. The
single- and multi-planar listings of a format are merged.

| Field | Present | Meaning |
| --- | --- | --- |
| `format` | always | V4L2 FourCC, e.g. `MJPG`, `YUYV`, `NV12` |
| `format_description` | when the driver gives one | The `VIDIOC_ENUM_FMT` description, e.g. `Motion-JPEG` |
| `width`, `height` | discrete sizes | Frame size |
| `size_range` | stepwise or continuous sizes | `type` (`stepwise` or `continuous`), `min_width`, `min_height`, `max_width`, `max_height`, `step_width`, `step_height` |
| `frame_intervals` | always | Per probed size (`width`, `height`), every interval the device advertises: `{"type": "discrete", "numerator", "denominator"}` or `{"type": "stepwise" or "continuous", "minimum", "maximum", "step"}` |

A size range is probed at its minimum and maximum size. A size without frame
intervals has no mode. Every mode also has `available` and, when it is false,
`reason` (see [the catalog](../api.md)). Neat's `UsbCameraInput` opens a
`uvcvideo` camera by its `camera_name` and captures MJPEG at the sizes and frame
intervals the camera lists, so an `MJPG` mode of such a camera is available
unless an earlier node of the camera lists it (see below). Other formats are
unavailable, and so is every mode of a camera of another driver. The `MJPG`
modes of a `uvcvideo` camera without `camera_name` are unavailable, since
`UsbCameraInput` cannot open it; those of a node without `driver`, whose
interface was binding or unbinding during the scan, have no verdict.

A camera with several streaming interfaces has several capture nodes on its
VideoControl interface, and they share its `camera_name`; a device with several
VideoControl interfaces has a `camera_name` for each. `UsbCameraInput` tries a
camera's nodes by device path, shorter first (`/dev/video2` before
`/dev/video10`), and opens the first one that lists the requested MJPEG size
and rate, a discrete interval exactly or a range within its bounds. An `MJPG`
mode is therefore unavailable when earlier nodes of the camera list its size at
every one of its frame intervals, and has no verdict when they list it at only
some of them.

## Limits and errors

A failure fails the provider's scan, which is reported in the catalog's
`errors`:

- `io.permission_denied`: `EACCES` from a node or sysfs, or `EPERM` from
  opening a node.
- `io.open`: any other driver or sysfs error; a list longer than 1024 entries;
  more than 4096 enumeration queries on one device; a malformed size or
  interval.
- `peripherals.discovery_failed`: a USB camera without its interface number or
  node index.

A camera unplugged during the scan is left out rather than failing it, at the
latest by the rescan its removal triggers.

## Example

An illustrative record; its sysfs topology and ACPI path are simplified.

```json
{"type": "camera", "id": "camera:v4l2:295faa7ac0d61654", "backend": "v4l2",
 "model": "UVC Webcam", "device_path": "/dev/video97",
 "by_id_path": "/dev/v4l/by-id/usb-1a2b_UVC_Webcam_A1B2-video-index0",
 "identity": {"stable_key": "sysfs:devices/pci0000:00/usb1/1-2.3:interface=00:index=0",
              "topology": "devices/pci0000:00/usb1/1-2.3", "interface": "00",
              "node_index": "0", "vendor_id": "1a2b", "product_id": "3c4d",
              "serial": "A1B2", "manufacturer": "Example Vendor", "speed": "480",
              "driver": "uvcvideo", "camera_name": "\\_SB_.PCI0-2.3:1.0-1a2b:3c4d"},
 "availability": {"state": "unknown", "reason": "V4L2 does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "MJPG", "format_description": "Motion-JPEG",
            "width": 1920, "height": 1080,
            "frame_intervals": [{"width": 1920, "height": 1080, "intervals": [
              {"type": "discrete", "numerator": 1, "denominator": 30}]}],
            "available": true}]}
```
