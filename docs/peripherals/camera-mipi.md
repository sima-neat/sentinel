# MIPI CSI-2 cameras

The `camera.mipi` provider reports the image sensors behind the Modalix ISP.
It rescans on `media` and `video4linux` uevents. Discovery opens device nodes
read-only and issues query ioctls only; it never sets a format or link, and
never streams.

Each `/dev/mediaN` of the `simaai-v4l2-vid` driver is read with
`MEDIA_IOC_G_TOPOLOGY`, and each `MEDIA_ENT_F_CAM_SENSOR` entity in its graph is
one camera. Other drivers' media devices (for example `uvcvideo`) are ignored.

## Identity

`id` is `camera:<sensor entity name>`, for example `camera:imx477 5-001a`: the
sensor driver, I2C bus and address. libcamera uses the same name, so it is
also the name `CameraInput` accepts. Two sensors with the same name, for
example on two media devices, would share an id, so the scan fails with an
error that names both media devices instead.

## Fields

| Field | Present | Meaning |
| --- | --- | --- |
| `type`, `id` | always | `camera`, and the identity above |
| `backend` | always | `mipi` |
| `model` | when the name has one | The first word of the entity name, e.g. `imx477` |
| `availability` | always | `{"state": "unknown", "reason": ...}`: the media controller has no read-only ownership state |
| `camera_name` | always | The sensor entity name, to pass to `CameraInput` |
| `media_device` | always | `/dev/mediaN` (routing only; not identity) |
| `bus_info` | when reported | The media device bus from `MEDIA_IOC_DEVICE_INFO`, e.g. `platform:csi2video@1` |
| `isp` | always | `{"state": "available", "device_path", "device_paths", "sizing"}`, or `{"state": "unavailable", "reason"}` with `modes: []` |
| `csi_receiver` | when linked | The entity the sensor's source pad links to, e.g. `csidev-40c3000.csi` |
| `sensor_timing` | when readable | `pixel_rate` (pixels/s), `hblank_min`, `vblank_min`, `width`, `height`, read from the sensor's `/dev/v4l-subdevN` |
| `max_fps` | with `sensor_timing` | `pixel_rate / ((width + hblank_min) * (height + vblank_min))`, to two decimals |

`sensor_timing` comes from the sensor's sub-device interface (named through
`/sys/dev/char/<major>:<minor>`): the active format of the linked source pad
(`VIDIOC_SUBDEV_G_FMT`), the current `V4L2_CID_PIXEL_RATE`, and the minimums of
`V4L2_CID_HBLANK` and `V4L2_CID_VBLANK`. It is omitted when any of them is
missing.

The ISP output nodes are the `video4linux` entries named
`isp_v4l2-vid-cap-out` with card `arm-isp-out`. When several are present, only
the modes they all share are reported.

`sizing` says where the ISP's output sizes come from, decided by what the ISP
lists rather than a platform version. An ISP that sets its sizes at run time
from the sensor it is configured for (Platform 3.0) lists 0x0 until then: when
any ISP node lists a 0x0 size, `sizing` is `runtime`. Otherwise (Platform
2.1.x) the sizes are a table built into the driver: `sizing` is `fixed`.

## Modes

Modes are the ISP output formats and discrete sizes, which is what
`CameraInput` can capture. Each mode has `format` (FourCC), `width`, `height`
and `isp_output: true`. A mode carries `frame_intervals`, in the same form as
for USB cameras, only when the ISP reports intervals for its size. The
DevKit's ISP reports none, so its modes have no frame rate; `max_fps` and
`sensor_timing` describe the sensor's limit.

With `sizing: "runtime"` the ISP lists only the size it is currently
configured for (0x0 at rest), so those sizes are not used. The modes are then
the formats every ISP node lists, at each discrete frame size the sensor's
sub-device reports over all its media-bus codes
(`VIDIOC_SUBDEV_ENUM_MBUS_CODE`, `VIDIOC_SUBDEV_ENUM_FRAME_SIZE`). Each has
`format`, `width`, `height` and `sensor_mode: true`, and carries
`frame_intervals` when the sensor reports intervals for the size
(`VIDIOC_SUBDEV_ENUM_FRAME_INTERVAL`). Ranges are skipped. When the sensor's
sizes cannot be listed, `isp` is unavailable for that camera, with the reason.

Every mode also has `available` and, when it is false, `reason`: whether the
board is set up for it. The rule is in [the catalog](../api.md).

## Errors

| Code | When |
| --- | --- |
| `io.permission_denied` | A media device cannot be opened (`EACCES`, `EPERM`), or `/dev` cannot be listed (`EACCES`) |
| `io.open` | Any other open or query failure of a media device |
| `peripherals.discovery_failed` | An unnamed sensor entity, or two sensors with the same name (their ids would collide) |

A device that disappears mid-scan is skipped. ISP failures never fail the scan;
they make `isp` unavailable with the reason.

## Example

The IMX477 on a Modalix DevKit, with modes abbreviated (nine in all: three
formats, three sizes). The graph and ISP sizes are transcribed from the
DevKit; the sensor-timing values are as the DevKit reports them but are
covered by tests only.

```json
{"type": "camera", "id": "camera:imx477 5-001a", "model": "imx477",
 "availability": {"state": "unknown", "reason": "The media controller does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "AR24", "width": 1920, "height": 1080, "isp_output": true},
           {"format": "AR24", "width": 2048, "height": 1080, "isp_output": true}],
 "backend": "mipi", "camera_name": "imx477 5-001a", "media_device": "/dev/media0",
 "bus_info": "platform:csi2video@1",
 "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"],
         "sizing": "fixed"},
 "csi_receiver": "csidev-40c3000.csi",
 "sensor_timing": {"pixel_rate": 840000000, "hblank_min": 9332, "vblank_min": 48,
                   "width": 1920, "height": 1080},
 "max_fps": 66.18}
```
