# USB 카메라

`camera.v4l2` 프로바이더가 V4L2를 통해 탐지하는 USB Video Class(UVC) 카메라입니다.
`video4linux` uevent가 발생할 때마다 다시 스캔합니다.

프로바이더는 `/sys/class/video4linux`를 순회하며 USB
상위 장치가 있는 노드만 유지하므로 플랫폼 및 ISP 노드는 절대 열지 않습니다. 각 후보는
`O_RDONLY | O_NONBLOCK`로 열리며 쿼리 ioctl만 받습니다(`VIDIOC_QUERYCAP`,
`VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES`, `VIDIOC_ENUM_FRAMEINTERVALS`).
메타데이터 전용, 출력 전용 및 메모리 간(memory-to-memory) 노드는
`VIDIOC_QUERYCAP` 이후 제외되므로, 메타데이터 노드가 있는 UVC 카메라도 한 번만 나타납니다.

## 식별 정보

`id`는 `camera:v4l2:<16 hex digits>` 형식이며, `identity.stable_key`
(`sysfs:<USB topology>:interface=<bInterfaceNumber>:index=<node index>`)의 FNV-1a 해시입니다. 같은 포트에 다시
연결하거나 `/dev/videoN` 번호가 다시 매겨져도 변하지 않습니다.
다른 포트에 연결하면 다른 카메라로 간주되며, 서로 다른 포트에 있는 동일한 카메라는
서로 다른 id를 갖습니다.

## 필드

| 필드 | 포함 조건 | 의미 |
| --- | --- | --- |
| `type` | 항상 | `camera` |
| `id` | 항상 | 위 참조 |
| `backend` | 항상 | `v4l2` (USB 카메라) |
| `model` | 알려진 경우 | USB `product` 문자열, 없으면 드라이버의 카드 이름 |
| `device_path` | 항상 | `/dev/videoN`; 라우팅 전용이며 식별자가 아님 |
| `by_id_path` | udev 링크가 있는 경우 | `device_path`로 확인되는 `/dev/v4l/by-id/...` 링크 |
| `identity` | 항상 | `stable_key`, `topology`, `interface`, `node_index`, 그리고 USB 장치가 보고하는 경우 `vendor_id`, `product_id`, `serial`, `manufacturer` 및 `speed`(커널이 출력하는 그대로의 Mb/s 단위 sysfs 속도, 예: `"480"`) |
| `availability` | 항상 | `{"state": "unknown", "reason": ...}`: 탐지는 스트림을 열지 않으므로 카메라가 사용 중인지 알 수 없음 |
| `modes` | 항상 | 카메라가 출력할 수 있는 항목. 아래 참조 |

## 모드

형식과 프레임 크기마다 하나의 모드가 있으며, 형식순으로 정렬한 다음 크기순으로 정렬합니다.
한 형식의 단일 평면 및 다중 평면 목록은 병합됩니다.

| 필드 | 포함 조건 | 의미 |
| --- | --- | --- |
| `format` | 항상 | V4L2 FourCC, 예: `MJPG`, `YUYV`, `NV12` |
| `format_description` | 드라이버가 제공하는 경우 | `VIDIOC_ENUM_FMT` 설명, 예: `Motion-JPEG` |
| `width`, `height` | 이산 크기 | 프레임 크기 |
| `size_range` | 단계형 또는 연속 크기 | `type` (`stepwise` 또는 `continuous`), `min_width`, `min_height`, `max_width`, `max_height`, `step_width`, `step_height` |
| `frame_intervals` | 항상 | 프로브한 크기(`width`, `height`)마다 장치가 알리는 모든 간격: `{"type": "discrete", "numerator", "denominator"}` 또는 `{"type": "stepwise" or "continuous", "minimum", "maximum", "step"}` |

크기 범위는 최소 크기와 최대 크기에서 프로브합니다. 프레임
간격이 없는 크기에는 모드가 없습니다. Sentinel은 모드의 지원 여부를 판단하지 않습니다.
`CameraInput`의 경우 Neat Core가 이를 결정합니다.

## 제한 및 오류

실패가 발생하면 프로바이더의 스캔이 실패하며, 이는 카탈로그의
`errors`에 보고됩니다:

- `io.permission_denied`: 노드나 sysfs에서 발생한 `EACCES`, 또는 다음 작업에서 발생한 `EPERM`:
  노드 열기.
- `io.open`: 그 밖의 드라이버 또는 sysfs 오류, 1024개를 초과하는 항목이 있는 목록,
  한 장치에 대한 4096회를 초과하는 열거 쿼리, 잘못된 형식의 크기 또는
  간격.
- `peripherals.discovery_failed`: USB 카메라에 인터페이스 번호 또는
  노드 인덱스가 없는 경우.

스캔 중에 분리된 카메라는 스캔을 실패시키지 않고 결과에서 제외되며, 늦어도
분리로 인해 발생하는 재스캔에서 제외됩니다.

## 예시

```json
{"type": "camera", "id": "camera:v4l2:295faa7ac0d61654", "backend": "v4l2",
 "model": "HD Pro Webcam C920", "device_path": "/dev/video97",
 "by_id_path": "/dev/v4l/by-id/usb-046d_HD_Pro_Webcam_C920_A1B2-video-index0",
 "identity": {"stable_key": "sysfs:devices/pci0000:00/usb1/1-2.3:interface=00:index=0",
              "topology": "devices/pci0000:00/usb1/1-2.3", "interface": "00",
              "node_index": "0", "vendor_id": "046d", "product_id": "082d",
              "serial": "A1B2", "manufacturer": "Logitech", "speed": "480"},
 "availability": {"state": "unknown", "reason": "V4L2 does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "MJPG", "format_description": "Motion-JPEG",
            "width": 1920, "height": 1080,
            "frame_intervals": [{"width": 1920, "height": 1080, "intervals": [
              {"type": "discrete", "numerator": 1, "denominator": 30}]}]}]}
```
