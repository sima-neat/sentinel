# 카메라

Neat 애플리케이션이 캡처할 수 있는 이미지 센서, 즉 Modalix ISP 뒤의 MIPI CSI-2 센서와
USB Video Class(UVC) 카메라를 다룹니다.

- **유형 토큰:** `camera`
- **공급자:** `daemon.camera.mipi`(내장), `daemon.camera.v4l2`(내장, USB)
- **재검색 트리거:** `media`, `video4linux`

## 식별자

| 카메라 | `id` | 구성 요소 |
| --- | --- | --- |
| MIPI | `camera:<sensor entity name>`, 예: `camera:imx477 5-001a` | 센서의 media-controller 엔터티 이름: 드라이버, I2C 버스와 주소. libcamera도 같은 이름을 사용하므로 `CameraInput`이 받는 이름이기도 합니다. |
| USB | `camera:v4l2:<16 hex digits>` | USB 토폴로지(버스 경로, 인터페이스, 인터페이스 내 노드 인덱스)의 해시. 같은 포트로 재연결하거나 `/dev/videoN` 번호가 바뀌어도 같고, 포트가 다르면 다른 카메라입니다. |

## 세부 정보: 공통

| 필드 | 유형 | 항상 존재 | 의미 |
| --- | --- | --- | --- |
| `backend` | string | yes | `mipi` 또는 `v4l2` |
| `connection` | string | yes | `mipi-csi2` 또는 `usb` |
| `model` | string | no | 센서 또는 제품 모델 |
| `availability` | object | yes | `{"state": "unknown", "reason": ...}`. 검색은 스트림을 열지 않으므로 카메라가 사용 중인지 알 수 없습니다. |
| `modes` | array | yes | 카메라가 출력할 수 있는 모드. 아래 참조 |

## 세부 정보: MIPI 전용

| 필드 | 유형 | 항상 존재 | 의미 | 출처 |
| --- | --- | --- | --- | --- |
| `camera_name` | string | yes | `CameraInput`에 전달할 이름 | `simaai-v4l2-vid` 미디어 장치의 센서 엔터티 이름 |
| `media_device` | string | yes | 미디어 장치 노드. 예: `/dev/media0`(라우팅 전용, 식별자 아님) | `/dev/media*` |
| `bus_info` | string | no | 미디어 장치 버스. 예: `platform:csi2video@1` | `MEDIA_IOC_DEVICE_INFO` |
| `isp` | object | yes | `{"state": "available", "device_path", "device_paths"}`, 또는 `{"state": "unavailable", "reason"}`와 `modes: []` | ISP 출력 노드(`isp_v4l2-vid-cap-out`, 카드 `arm-isp-out`) |
| `csi_receiver` | string | no | 센서가 공급하는 CSI-2 수신기 엔터티. 예: `csidev-40c3000.csi` | `MEDIA_IOC_G_TOPOLOGY`에서 센서 소스 패드 데이터 링크의 반대쪽 엔터티(활성 링크 우선, 그다음 가장 낮은 패드 인덱스) |
| `sensor_timing` | object | no | `pixel_rate`(pixels/s), `hblank_min`, `vblank_min`, `width`, `height` | 센서의 `/dev/v4l-subdevN`(`/sys/dev/char/<major>:<minor>`로 이름을 얻는 인터페이스 링크)을 읽기 전용으로 열어 연결된 소스 패드의 활성 형식(`VIDIOC_SUBDEV_G_FMT`), 현재 `V4L2_CID_PIXEL_RATE`(`VIDIOC_G_EXT_CTRLS`), `V4L2_CID_HBLANK`와 `V4L2_CID_VBLANK`의 최솟값(`VIDIOC_QUERY_EXT_CTRL`)을 읽음. 하위 장치, 형식 또는 제어가 없으면 생략 |
| `max_fps` | number | `sensor_timing`과 함께 | 활성 형식에서 센서의 프레임 속도 한계(소수점 둘째 자리) | `pixel_rate / ((width + hblank_min) * (height + vblank_min))` |

## 세부 정보: USB 전용

| 필드 | 유형 | 항상 존재 | 의미 |
| --- | --- | --- | --- |
| `device_path` | string | no | `/dev/videoN`(라우팅 전용, 식별자 아님) |
| `identity` | object | yes | `stable_key`, `topology`, `interface`, `node_index`, `vendor_id`, `product_id`와, 장치가 보고할 경우 `serial`, `manufacturer`, `speed`(커널이 출력하는 USB sysfs 속도 Mb/s. 예: `"480"`, `"5000"`) |
| `by_id_path` | string | no | udev `/dev/v4l/by-id/...` 링크이며 `device_path`로 해석됨. udev 링크가 없으면 생략(라우팅 전용, 식별자 아님) |

## 모드

| 필드 | 유형 | 존재 조건 | 의미 |
| --- | --- | --- | --- |
| `format` | string | always | V4L2 FourCC. 예: `NV12`, `RGB3`, `AR24`, `MJPG`, `YUYV` |
| `format_description` | string | USB, 드라이버가 제공할 때 | 드라이버의 `VIDIOC_ENUM_FMT` 설명. 예: `Motion-JPEG`, `YUYV 4:2:2` |
| `width`, `height` | integer | 이산 크기 | 프레임 크기 |
| `size_range` | object | 범위(USB) | `min_width`, `min_height`, `max_width`, `max_height`, `step_width`, `step_height` |
| `framerate_num`, `framerate_den` | integer | always | 프레임 속도. USB는 알린 간격 중 가장 빠른 값을 사용 |
| `frame_intervals` | array | USB | 장치가 알리는 모든 간격 |
| `isp_output` | bool | MIPI | `true`: ISP 출력 크기 |
| `framerate_source` | string | MIPI | `isp`(ISP의 이산 프레임 간격 또는 간격 범위에서 가장 빠른 유효 속도), `sensor_timing`(`max_fps`까지 제공하는 속도), 또는 `nominal`(30/1: 둘 다 알 수 없음) |
| `supported`, `reason` | bool, string | always | Neat Core 규칙에 따라 지원 단계에서 추가 |

MIPI 모드는 ISP 출력 노드의 형식과 이산 크기이며 `CameraInput`이 실제로 캡처할 수 있는 값입니다.
libcamera는 ISP가 생성할 수 없는 크기를 포함한 더 긴 목록을 알립니다(sima-neat/core#883 참조).

크기별 MIPI 프레임 속도는 ISP가 이산 프레임 간격을 제공하면 그 값을 사용하거나, 단계형/연속형 간격 범위에서는 가장 빠른 유효 속도를 사용합니다. 그렇지 않고 `max_fps`를
알 수 있으면 `max_fps`를 가장 가까운 정수(최소 1)로 반올림한 속도와 그 이하의 표준 속도 60, 30, 25,
20, 15, 10, 5를 빠른 순서로 제공합니다(66.18이면 66, 60, 30, 25, 20, 15, 10, 5).
그 외에는 공칭 30/1 모드 하나를 사용합니다. `max_fps`는 센서의 활성 형식에서의 한계이며 모든 ISP 크기에
적용됩니다. 다른 센서 모드가 필요한 크기는 더 느릴 수 있습니다.

## 레코드 예시

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

USB 카메라 세부 정보의 축약 예시:

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

## 지원하는 변동

- 한 미디어 장치의 여러 센서와 여러 SiMa 미디어 장치. 각각 자체 수신기와 타이밍을 가짐.
- 하위 장치 노드, 읽을 수 있는 활성 형식, pixel-rate 또는 blanking 제어가 없는 센서
  (`sensor_timing` 없음, 공칭 속도). 링크가 없거나 상한을 넘는 링크가 있거나 media API가 4.19보다 오래된
  그래프(패드 인덱스가 없어 `sensor_timing` 없음). 여러 소스 패드가 있는 센서(연결되고 활성화된 패드 사용).
- 센서가 없는 미디어 장치와 다른 드라이버의 미디어 장치(무시).
- ISP 노드가 없거나 읽을 수 없거나 프레임 간격을 보고하거나 여러 개 존재하는 경우
  (모든 ISP 노드가 공유하는 모드만 보고).
- manufacturer, speed, `/dev/v4l/by-id` 링크가 있거나 없는 USB 카메라, 해석되지 않는 by-id 링크,
  드라이버 설명이 있거나 없는 형식.
- 여러 동일 USB 카메라, 일련번호가 없는 카메라, 복합 장치(카메라와 마이크), 메타데이터 전용 및 출력 전용
  비디오 노드(제외), 이산·단계형·연속형 크기와 간격.
- 스캔 사이의 장치 노드 번호 변경.

## 지원 규칙

Neat Core는 `/usr/share/simaai-sentinel/support/neat-core.json`을 설치합니다. 카메라 규칙은
`backend`, `format`, 프레임 속도, 크기 범위(항상 미지원), `isp_output` 순으로 검사합니다.
Neat Core가 없으면 모든 모드는 `supported: false`이고 이유는 "Neat Core is not installed"입니다.
[지원 규칙](../README.md#support-rules)을 참조하십시오.

## 검증

| 동작 | 실제 하드웨어 | 픽스처만 사용 |
| --- | --- | --- |
| IMX477 이름, 미디어 그래프와 ISP 크기 | DevKit 캡처(2.1.3)에서 옮김 | |
| `/dev/media*` 및 ISP 노드를 통한 실시간 검색: libcamera와 이름 일치, ISP 노드 16개, 모드 9개, NV12 지원 | DevKit, 2026-10-03 | 합성 커널 인터페이스 |
| Logitech C920: 레코드 1개, 메타데이터 노드 제외, `v4l2-ctl`과 일치하는 17 MJPG / 18 YUYV 모드, 재연결 후 같은 id | DevKit, 2026-10-03 | 합성, v1 카탈로그 픽스처와 일치 |
| 1920x1080 스트림 중 검색해도 프레임이 누락되지 않음 | DevKit, 2026-10-03 | |
| 핫플러그, 여러 카메라, 번호 변경 | | 합성 |
| `csi_receiver`, `sensor_timing`, `max_fps` 및 sensor-timing 속도 | 아직 확인하지 않음. 테스트의 IMX477 값(840 MHz, HBLANK 9332, VBLANK 48, 1920x1080, 66.18 fps)은 DevKit 보고값이며 65~66 fps 측정. 그래프 링크는 `media-ctl -p`에서 옮김 | 합성 하위 장치와 제어, 합성 장치 번호와 그래프 id |
| USB `manufacturer`, `speed`, `by_id_path`, `format_description` | 아직 확인하지 않음 | 합성 sysfs, udev 링크와 드라이버 설명 |
