# MIPI CSI-2 카메라

`camera.mipi` 프로바이더는 Modalix ISP 뒤에 있는 이미지 센서를 보고합니다.
`media` 및 `video4linux` uevent가 발생하면 다시 스캔합니다. 탐지는 장치 노드를
읽기 전용으로 열고 쿼리 ioctl만 실행합니다. 형식이나 링크를 설정하지 않으며
스트리밍도 하지 않습니다.

`simaai-v4l2-vid` 드라이버의 각 `/dev/mediaN`는
`MEDIA_IOC_G_TOPOLOGY`로 읽으며, 그래프의 각 `MEDIA_ENT_F_CAM_SENSOR` 엔티티가
하나의 카메라입니다. 다른 드라이버의 미디어 장치(예: `uvcvideo`)는 무시됩니다.

## 식별 정보

`id`는 `camera:<sensor entity name>`이며, 예를 들어 `camera:imx477 5-001a`는
센서 드라이버, I2C 버스 및 주소로 구성됩니다. libcamera도 같은 이름을 사용하므로
`CameraInput`이 받는 이름이기도 합니다. 예를 들어 두 미디어 장치에 같은 이름의
센서가 있으면 id가 겹치게 되므로, 이 경우 두 미디어 장치를 모두 명시하는
오류와 함께 스캔이 실패합니다.

## 필드

| 필드 | 포함 조건 | 의미 |
| --- | --- | --- |
| `type`, `id` | 항상 | `camera` 및 위의 식별자 |
| `backend` | 항상 | `mipi` |
| `model` | 이름에 포함된 경우 | 엔티티 이름의 첫 단어, 예: `imx477` |
| `availability` | 항상 | `{"state": "unknown", "reason": ...}`: 미디어 컨트롤러에는 읽기 전용 소유권 상태가 없음 |
| `camera_name` | 항상 | `CameraInput`에 전달할 센서 엔티티 이름 |
| `media_device` | 항상 | `/dev/mediaN` (라우팅 전용이며 식별자가 아님) |
| `bus_info` | 보고된 경우 | `MEDIA_IOC_DEVICE_INFO`에서 얻은 미디어 장치 버스, 예: `platform:csi2video@1` |
| `isp` | 항상 | `{"state": "available", "device_path", "device_paths"}`, 또는 `{"state": "unavailable", "reason"}` (이때 `modes: []`) |
| `csi_receiver` | 링크된 경우 | 센서의 소스 패드가 연결된 엔티티, 예: `csidev-40c3000.csi` |
| `sensor_timing` | 읽을 수 있는 경우 | 센서의 `/dev/v4l-subdevN`에서 읽은 `pixel_rate`(픽셀/초), `hblank_min`, `vblank_min`, `width`, `height` |
| `max_fps` | `sensor_timing`이 있는 경우 | `pixel_rate / ((width + hblank_min) * (height + vblank_min))`, 소수점 둘째 자리까지 |

`sensor_timing`은 센서의 서브디바이스 인터페이스(이름은
`/sys/dev/char/<major>:<minor>`로 확인)에서 가져옵니다. 연결된 소스 패드의 활성 형식
(`VIDIOC_SUBDEV_G_FMT`), 현재 `V4L2_CID_PIXEL_RATE`, 그리고
`V4L2_CID_HBLANK` 및 `V4L2_CID_VBLANK`의 최솟값입니다. 이 중 하나라도
없으면 생략됩니다.

ISP 출력 노드는 이름이
`isp_v4l2-vid-cap-out`이고 카드가 `arm-isp-out`인 `video4linux` 항목입니다. 여러 개가 있으면
모두가 공유하는 모드만 보고됩니다.

## 모드

모드는 ISP 출력 형식과 이산 크기이며, 이는
`CameraInput`가 캡처할 수 있는 것입니다. 각 모드에는 `format`(FourCC), `width`, `height`
및 `isp_output: true`가 있습니다. 모드는 ISP가 해당 크기의 간격을 보고하는 경우에만
USB 카메라와 같은 형식의 `frame_intervals`를 포함합니다.
DevKit의 ISP는 간격을 보고하지 않으므로 모드에 프레임 레이트가 없습니다. `max_fps` 및
`sensor_timing`이 센서의 한계를 나타냅니다.

## 오류

| 코드 | 발생 조건 |
| --- | --- |
| `io.permission_denied` | 미디어 장치를 열 수 없거나(`EACCES`, `EPERM`) `/dev` 목록을 가져올 수 없음(`EACCES`) |
| `io.open` | 미디어 장치의 그 밖의 열기 또는 쿼리 실패 |
| `peripherals.discovery_failed` | 이름 없는 센서 엔티티, 또는 이름이 같은 두 센서(id가 충돌함) |

스캔 중에 사라진 장치는 건너뜁니다. ISP 실패로 스캔이 실패하지는 않으며,
대신 `isp`를 사용 불가 상태로 만들고 그 이유를 함께 표시합니다.

## 예시

Modalix DevKit의 IMX477이며, 모드는 축약되어 있습니다(전체 9개: 세 가지
형식, 세 가지 크기). 그래프와 ISP 크기는
DevKit에서 옮겨 적은 것입니다. 센서 타이밍 값은 DevKit이 보고하는 값과 같지만
테스트로만 검증되었습니다.

```json
{"type": "camera", "id": "camera:imx477 5-001a", "model": "imx477",
 "availability": {"state": "unknown", "reason": "The media controller does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "AR24", "width": 1920, "height": 1080, "isp_output": true},
           {"format": "AR24", "width": 2048, "height": 1080, "isp_output": true}],
 "backend": "mipi", "camera_name": "imx477 5-001a", "media_device": "/dev/media0",
 "bus_info": "platform:csi2video@1",
 "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"]},
 "csi_receiver": "csidev-40c3000.csi",
 "sensor_timing": {"pixel_rate": 840000000, "hblank_min": 9332, "vblank_min": 48,
                   "width": 1920, "height": 1080},
 "max_fps": 66.18}
```
