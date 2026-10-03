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

## Details: USB only

| Field | Type | Always present | Meaning |
| --- | --- | --- | --- |
| `device_path` | string | no | `/dev/videoN` (routing only; not identity) |
| `identity` | object | yes | `stable_key`, `topology`, `interface`, `node_index`, `vendor_id`, `product_id`, and `serial` when the device reports one |

## Modes

| Field | Type | Present | Meaning |
| --- | --- | --- | --- |
| `format` | string | always | V4L2 FourCC, e.g. `NV12`, `RGB3`, `AR24`, `MJPG`, `YUYV` |
| `width`, `height` | integer | discrete sizes | Frame size |
| `size_range` | object | ranges (USB) | `min_width`, `min_height`, `max_width`, `max_height`, `step_width`, `step_height` |
| `framerate_num`, `framerate_den` | integer | always | Frame rate; USB uses the fastest advertised interval |
| `frame_intervals` | array | USB | Every interval the device advertises |
| `isp_output` | bool | MIPI | `true`: an ISP output size |
| `framerate_source` | string | MIPI | `nominal` (30/1: the ISP reports no intervals) or `isp` |
| `supported`, `reason` | bool, string | always | Added by the support stage from Neat Core's rules |

MIPI modes are the ISP output node's formats and discrete sizes, which is what
`CameraInput` can actually capture. libcamera advertises a longer list that
includes sizes the ISP cannot produce (see sima-neat/core#883).

## Example record

```json
{"id": "camera:imx477 5-001a", "type": "camera", "provider": "daemon.camera.mipi",
 "camera": {"camera_name": "imx477 5-001a", "model": "imx477", "backend": "mipi",
            "connection": "mipi-csi2", "media_device": "/dev/media0",
            "bus_info": "platform:csi2video@1",
            "availability": {"state": "unknown", "reason": "..."},
            "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"]},
            "modes": [{"format": "NV12", "width": 1920, "height": 1080,
                       "framerate_num": 30, "framerate_den": 1, "framerate_source": "nominal",
                       "isp_output": true, "supported": true, "reason": ""}]}}
```

## Support rules

Neat Core's rules check, in order: `backend`, `format`, frame rate, size ranges
(never supported), and `isp_output`. See [support rules](../README.md#support-rules).

## Verification

| Behaviour | Real hardware | Fixtures only |
| --- | --- | --- |
| IMX477: name matches libcamera, 16 ISP nodes, 9 modes, NV12 supported | DevKit, 2026-10-03 | Transcribed capture |
| Logitech C920: one record, metadata node excluded, 17 MJPG / 18 YUYV modes, same id after unplug and replug | DevKit, 2026-10-03 | |
| Discovery during a 1920x1080 stream does not drop frames | DevKit, 2026-10-03 | |
| Several cameras, renumbering, missing strings, ranges | | Synthetic |
| Frame-rate limits from sensor timing | Not implemented (rate is nominal) | |
