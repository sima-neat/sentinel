# Cameras

Image sensors that a Neat application can capture from: MIPI CSI-2 sensors
behind the Modalix ISP, and USB Video Class (UVC) cameras.

- **Type token:** `camera`
- **Providers:** `daemon.camera.mipi` (built-in), `daemon.camera.v4l2` (built-in, USB)
- **Rescan triggers:** `media`, `video4linux`

## Identity

| Camera | `id` | Built from |
| --- | --- | --- |
| MIPI | `camera:<sensor entity name>`, e.g. `camera:imx477 5-001a` | The sensor's media-controller entity name: driver, I2C bus and address. libcamera uses the same name, so it is also the name `CameraInput` accepts. |
| USB | `camera:v4l2:<16 hex digits>` | A hash of the USB topology (bus path, interface, node index within the interface). Unchanged across replugs into the same port and `/dev/videoN` renumbering; a different port is a different camera. |

## Details: both kinds

| Field | Type | Always present | Meaning |
| --- | --- | --- | --- |
| `backend` | string | yes | `mipi` or `v4l2` |
| `connection` | string | yes | `mipi-csi2` or `usb` |
| `model` | string | no | Sensor or product model |
| `availability` | object | yes | `{"state": "unknown", "reason": ...}`. Discovery never opens a stream, so it cannot tell whether a camera is in use. |
| `modes` | array | yes | What the camera can output; see below |

## Details: MIPI only

| Field | Type | Always present | Meaning | Source |
| --- | --- | --- | --- | --- |
| `camera_name` | string | yes | Name to pass to `CameraInput` | Sensor entity name on the `simaai-v4l2-vid` media device |
| `media_device` | string | yes | Media device node, e.g. `/dev/media0` (routing only; not identity) | `/dev/media*` |
| `bus_info` | string | no | Media device bus, e.g. `platform:csi2video@1` | `MEDIA_IOC_DEVICE_INFO` |
| `isp` | object | yes | `{"state": "available", "device_path", "device_paths"}`, or `{"state": "unavailable", "reason"}` with `modes: []` | ISP output node (`isp_v4l2-vid-cap-out`, card `arm-isp-out`) |
| `csi_receiver` | string | no | The CSI-2 receiver entity the sensor feeds, e.g. `csidev-40c3000.csi` | The entity at the other end of the data link from the sensor's source pad (enabled links first, then the lowest pad index), from `MEDIA_IOC_G_TOPOLOGY` |
| `sensor_timing` | object | no | `pixel_rate` (pixels/s), `hblank_min`, `vblank_min`, `width`, `height` | The sensor's `/dev/v4l-subdevN` (its interface link, named through `/sys/dev/char/<major>:<minor>`), opened read-only: the active format of the linked source pad (`VIDIOC_SUBDEV_G_FMT`), the current `V4L2_CID_PIXEL_RATE` (`VIDIOC_G_EXT_CTRLS`), and the minimum of `V4L2_CID_HBLANK` and `V4L2_CID_VBLANK` (`VIDIOC_QUERY_EXT_CTRL`). Omitted when the sub-device, the format or any control is missing |
| `max_fps` | number | with `sensor_timing` | The sensor's frame-rate limit at its active format, to two decimals | `pixel_rate / ((width + hblank_min) * (height + vblank_min))` |

## Details: USB only

| Field | Type | Always present | Meaning |
| --- | --- | --- | --- |
| `device_path` | string | no | `/dev/videoN` (routing only; not identity) |
| `identity` | object | yes | `stable_key`, `topology`, `interface`, `node_index`, `vendor_id`, `product_id`, and, when the device reports them, `serial`, `manufacturer` and `speed` (the USB device's sysfs speed in Mb/s as the kernel prints it, e.g. `"480"`, `"5000"`) |
| `by_id_path` | string | no | The udev `/dev/v4l/by-id/...` link that resolves to `device_path`; absent without udev links (routing only; not identity) |

## Modes

| Field | Type | Present | Meaning |
| --- | --- | --- | --- |
| `format` | string | always | V4L2 FourCC, e.g. `NV12`, `RGB3`, `AR24`, `MJPG`, `YUYV` |
| `format_description` | string | USB, when the driver gives one | The driver's `VIDIOC_ENUM_FMT` description, e.g. `Motion-JPEG`, `YUYV 4:2:2` |
| `width`, `height` | integer | discrete sizes | Frame size |
| `size_range` | object | ranges (USB) | `min_width`, `min_height`, `max_width`, `max_height`, `step_width`, `step_height` |
| `framerate_num`, `framerate_den` | integer | always | Frame rate; USB uses the fastest advertised interval |
| `frame_intervals` | array | USB | Every interval the device advertises |
| `isp_output` | bool | MIPI | `true`: an ISP output size |
| `framerate_source` | string | MIPI | `isp` (an ISP frame interval), `sensor_timing` (a rate offered up to `max_fps`), or `nominal` (30/1: neither is known) |
| `supported`, `reason` | bool, string | always | Added by the support stage from Neat Core's rules |

MIPI modes are the ISP output node's formats and discrete sizes, which is what
`CameraInput` can actually capture. libcamera advertises a longer list that
includes sizes the ISP cannot produce (see sima-neat/core#883).

MIPI frame rates, per size: the ISP's discrete frame intervals when it lists
them. Otherwise, when `max_fps` is known, one mode per rate in `max_fps`
rounded to the nearest whole rate (at least 1) and every standard rate of 60,
30, 25, 20, 15, 10 and 5 below it, fastest first (66.18 gives 66, 60, 30, 25,
20, 15, 10, 5). Otherwise one nominal 30/1 mode. `max_fps` is the limit at
the sensor's active format and applies to every ISP size; a size that needs
another sensor mode may be slower.

## Example record

```json
{"id": "camera:imx477 5-001a", "type": "camera", "provider": "daemon.camera.mipi",
 "camera": {"camera_name": "imx477 5-001a", "model": "imx477", "backend": "mipi",
            "connection": "mipi-csi2", "media_device": "/dev/media0",
            "bus_info": "platform:csi2video@1", "csi_receiver": "csidev-40c3000.csi",
            "sensor_timing": {"pixel_rate": 840000000, "hblank_min": 9332, "vblank_min": 48,
                              "width": 1920, "height": 1080},
            "max_fps": 66.18,
            "availability": {"state": "unknown", "reason": "..."},
            "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"]},
            "modes": [{"format": "NV12", "width": 1920, "height": 1080,
                       "framerate_num": 66, "framerate_den": 1, "framerate_source": "sensor_timing",
                       "isp_output": true, "supported": false, "reason": "..."},
                      {"format": "NV12", "width": 1920, "height": 1080,
                       "framerate_num": 30, "framerate_den": 1, "framerate_source": "sensor_timing",
                       "isp_output": true, "supported": true, "reason": ""}]}}
```

A USB camera's details, abbreviated:

```json
{"model": "HD Pro Webcam C920", "backend": "v4l2", "connection": "usb",
 "device_path": "/dev/video0",
 "by_id_path": "/dev/v4l/by-id/usb-046d_HD_Pro_Webcam_C920_A1B2C3D4-video-index0",
 "identity": {"stable_key": "sysfs:devices/platform/.../usb1/1-1:interface=00:index=0",
              "topology": "devices/platform/.../usb1/1-1", "interface": "00", "node_index": "0",
              "vendor_id": "046d", "product_id": "082d", "serial": "A1B2C3D4",
              "manufacturer": "Logitech", "speed": "480"},
 "modes": [{"format": "MJPG", "format_description": "Motion-JPEG",
            "width": 1920, "height": 1080, "framerate_num": 30, "framerate_den": 1,
            "frame_intervals": [...], "supported": false, "reason": "..."}]}
```

## Variation covered

- Several sensors on one media device, and several SiMa media devices, each
  with its own receiver and timing.
- Sensors without a sub-device node, a readable active format, or any of the
  pixel-rate and blanking controls (no `sensor_timing`, nominal rates); graphs
  without links, with more links than the cap, or from a media API older than
  4.19 (no pad index, so no `sensor_timing`); a sensor with several source
  pads (the linked, enabled pad is used).
- Media devices without a sensor, and other drivers' media devices (ignored).
- ISP node missing, unreadable, reporting frame intervals, or present more than
  once (only the modes every ISP node shares are reported).
- USB cameras with and without manufacturer, speed and `/dev/v4l/by-id`
  links, a by-id link that resolves nowhere, and formats with and without a
  driver description.
- Several identical USB cameras, cameras without a serial, composite devices
  (camera plus microphone), metadata-only and output-only video nodes (excluded),
  discrete, stepwise and continuous sizes and intervals.
- Device nodes renumbered between scans.

## Support rules

Neat Core installs `/usr/share/simaai-sentinel/support/neat-core.json`. For
cameras the rules check, in order: `backend`, `format`, frame rate, size
ranges (never supported), and `isp_output`. Without Neat Core, every mode is
`supported: false` with the reason "Neat Core is not installed". See
[support rules](../README.md#support-rules).

## Verification

| Behaviour | Real hardware | Fixtures only |
| --- | --- | --- |
| IMX477 name, media graph and ISP sizes | Transcribed from a DevKit capture (2.1.3) | |
| Live discovery through `/dev/media*` and the ISP node: name matches libcamera, 16 ISP nodes, 9 modes, NV12 supported | DevKit, 2026-10-03 | Synthetic fake kernel interfaces |
| Logitech C920: one record, metadata node excluded, 17 MJPG / 18 YUYV modes matching `v4l2-ctl`, same id after unplug and replug | DevKit, 2026-10-03 | Synthetic, matching the v1 catalog fixture |
| Discovery during a 1920x1080 stream does not drop frames | DevKit, 2026-10-03 | |
| Hot-plug, several cameras, renumbering | | Synthetic |
| `csi_receiver`, `sensor_timing`, `max_fps` and sensor-timing rates | Not yet. The IMX477 values in the tests (840 MHz, HBLANK 9332, VBLANK 48, 1920x1080, 66.18 fps) are as reported from the DevKit, where 65-66 fps is measured; the graph's links are transcribed from `media-ctl -p` | Synthetic sub-device and controls; synthetic device numbers and graph ids |
| USB `manufacturer`, `speed`, `by_id_path`, `format_description` | Not yet | Synthetic sysfs, udev links and driver descriptions |
