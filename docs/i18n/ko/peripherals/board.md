# 보드 카메라 구성

카탈로그의 `board` 블록은 보드가 MIPI 카메라용으로 어떻게 설정되어 있는지
설명합니다. 어떤 보드인지, U-Boot가 어떤 오버레이를 적용하는지, 부팅된 디바이스 트리가
어떤 카메라 센서를 기술하는지, 설치된 오버레이가 어떤 센서를 기술할 수 있는지를
나타냅니다. 구성된 각 카메라는 자신의 센서에 해당하는 `camera.mipi` 카메라를 지정하므로,
클라이언트는 카메라의 `compatible` 값과,
`supported_sensors`에서 그 센서를 구성하는 오버레이를 찾을 수 있습니다. 장치 레코드와 마찬가지로
사실만 보고하며 구성을 변경하지 않습니다.

이 블록은 모든 스캔에서 프로바이더 다음에 읽으므로 `camera_id`는 항상
같은 카탈로그의 `devices`를 가리키며, 블록이 변경되면 `revision`도
바뀝니다. 첫 스캔이 완료될 때까지만 없으며, 탐지가 비활성화된 경우 API는
이전과 같이 HTTP 503을 반환합니다.

## 필드

| 필드 | 포함 조건 | 의미 |
| --- | --- | --- |
| `model` | 디바이스 트리에 모델이 있는 경우 | `/sys/firmware/devicetree/base/model`의 값으로, 첫 NUL 문자까지이며 앞뒤 공백은 제외합니다. 예: `SiMa.ai Modalix SoM 16Gig Board` |
| `overlays` | `fw_printenv -n dtbos`가 한 번 성공한 이후 | U-Boot `dtbos` 변수의 항목 중 `.dtbo`로 끝나는 항목을 순서대로 나열합니다. 카메라 오버레이뿐 아니라 U-Boot가 적용하는 모든 오버레이입니다(PCIe, 보안 부팅 및 플래시 오버레이도 같은 변수를 사용함). Sentinel은 어떤 항목이 카메라를 구성하는지 알려 주지 않습니다. 센서를 구성하는 오버레이는 `supported_sensors`에 나열됩니다. 이후 실행이 실패하면 마지막으로 성공한 실행의 목록이 유지되고 실패가 보고됩니다 |
| `configured_cameras` | 항상 | 라이브 디바이스 트리에서 I2C에 있는 MIPI CSI-2 센서. 비어 있을 수 있음 |
| `supported_sensors` | 항상 | `/boot` 아래의 오버레이 파일이 구성하는 센서. 비어 있을 수 있음 |

각 `configured_cameras` 항목:

| 필드 | 의미 |
| --- | --- |
| `compatible` | 노드의 첫 번째 `compatible` 문자열, 예: `sony,imx477` |
| `dt_node` | 디바이스 트리 루트로부터의 노드 경로, 예: `/i2cmux@0/i2c@0/imx477@1a` |
| `i2c_device` | sysfs에서 명명하는 `<bus>-<address>` 형식의 I2C 장치, 예: `5-001a` |
| `data_lanes` | 센서 엔드포인트의 `data-lanes`에 있는 셀 개수 |
| `camera_id` | 이 센서에 해당하는 `camera.mipi` 카메라의 `id`. 카탈로그에 해당 카메라가 없으면 없음 |

각 `supported_sensors` 항목에는 `compatible`과 `overlays`(해당 센서를 구성하는 오버레이의
정렬된 파일 이름)가 있습니다. 항목은
`compatible` 기준으로 정렬됩니다. 목록은 `/boot` 아래의 모든 슬롯 디렉터리를 병합하므로,
부팅되지 않은 슬롯에만 설치된 오버레이가 포함될 수 있습니다.

## 각 정보의 출처

- **라이브 디바이스 트리**는 U-Boot가
  오버레이를 적용한 뒤 커널이 부팅에 사용한 구성입니다. 실제 적용 중인 구성이므로 모델과
  구성된 카메라는 오버레이 파일이 아니라 여기에서 가져옵니다.
- **I2C `of_node` 링크**는 구성된 카메라를 찾고 매칭하는 방법입니다.
  `/sys/bus/i2c/devices`에 있는 I2C 클라이언트(이름은 `<bus>-<address>`이며,
  이름이 `i2c-N`인 어댑터는 건너뜀) 중 `of_node` 링크가 라이브 디바이스 트리 안으로
  확인되는 클라이언트는 모두 후보입니다. 노드에 `compatible`이 있고,
  다른 장치의 노드 안이 아닌 그 아래의 `endpoint` 노드에 MIPI CSI-2 소스임을 나타내는
  `data-lanes`가 있으면 구성된 카메라입니다. V4L2는 I2C 센서의 서브디바이스 이름을
  `<driver> <bus>-<address>`로 지정하며, 때로는 뒤에 다른 단어가 붙습니다
  (`ccs 5-0010 pixel_array`). 따라서 `camera_name`에
  `<i2c_device>`가 공백으로 구분된 온전한 단어로 포함된 카메라가 이 센서입니다. 이 방식은
  공급업체나 센서 이름을 비교하지 않고 정확한 장치를 매칭합니다.
- **오버레이 목록**은 U-Boot 환경에 보관되는
  플랫폼 자체의 기록입니다. `fw_printenv`는 보드에서 그 환경이 저장된 위치를
  알고 있으므로 Sentinel은 플래시를 직접 읽는 대신 이 도구에 묻습니다. 값은
  U-Boot가 다음 부팅 시 적용할 목록이며, 부팅 후 변경되지 않았다면
  부팅된 목록과 같습니다. 탐지 과정에서 커널 인터페이스를 읽는 대신 프로그램을 실행하는
  부분은 이것뿐입니다. `fw_printenv`는 root로 실행되며 환경을 변경하지 않지만, 환경의
  잠금을 획득합니다. DevKit에 포함된 libubootenv는 `/var/lock/fw_printenv.lock`을 만들고
  그 파일의 배타적 `flock`을 기다리므로, 동시에 실행 중인 `fw_setenv`가 있으면 스캔이
  타임아웃까지 대기할 수 있습니다.
- **오버레이 파일**은 플랫폼이 제공하는 오버레이입니다. 다음 위치의 모든 `*.dtbo`
  파일을 파싱합니다. 대상은 `/boot` 바로 아래의 디렉터리(예: A/B 슬롯인 `/boot/boot-0/` 및
  `/boot/boot-1/`)에 있는 파일입니다. 센서는 위와 같이 `compatible`과 CSI-2 엔드포인트를
  가진 노드로서, 오버레이가 I2C 버스에 추가하는 노드입니다. 즉 이름이 `i2c`로 시작하는
  노드 아래에 있거나, 대상이 그러한 노드인 프래그먼트 안에 있는 노드입니다. 대상은
  `target-path`로 지정되거나, 프래그먼트의 `target`이 참조하는 `__fixups__`의 레이블로
  지정됩니다. 여러 슬롯에서 발견된 파일 이름은 한 번만 나열됩니다.

아래에 또 다른 CSI-2 소스가 있는 CSI-2 소스는 브리지(예: I2C 먹스 또는 GMSL
디시리얼라이저와 시리얼라이저)이며 두 목록 모두에서 제외되므로
센서 자체만 나타납니다.

카메라 해상도와 형식은 여전히 ISP에서 가져오며 카메라의
`modes`에 나타납니다. 오버레이는 이를 기술하지 않습니다.

## 한도

탐지는 최대 I2C 장치 1024개를 읽고, 각 장치의 서브트리에서는 노드 256개, 노드마다 항목 256개,
장치 자신의 노드를 포함해 32단계의 노드까지 읽습니다. 또한 `/boot` 및 그 각 디렉터리의 항목 4096개,
그리고 각각 최대 1 MiB인 오버레이 파일 512개까지만 읽습니다. 이 한도에서 잘린 목록은 해당 필드의
오류로 보고되며, 읽은 부분은 그대로 게시됩니다. 오버레이 파일은 한 번 파싱되며, 크기, 수정 시간, 변경 시간 또는
inode가 바뀔 때만 다시 파싱됩니다. 읽지 못한 파일은
다음 스캔에서 다시 읽습니다. `fw_printenv`는 2초 후 강제 종료됩니다. 스캔은 종료를 최대
0.5초 더 기다린 뒤, 그 프로세스를 백그라운드에서 회수하도록 맡깁니다. 출력은 최대 64 KiB, 오류
출력은 최대 4 KiB까지만 읽습니다.

## 오류

각 오류는 그 오류로 인해 불완전해진 필드를 나타냅니다. `provider`는
`board.model`, `board.overlays`, `board.configured_cameras` 또는
`board.supported_sensors`입니다. 블록의 나머지 부분은 그대로 게시됩니다.

| 코드 | 발생 조건 |
| --- | --- |
| `io.open` | `model`, `/sys/bus/i2c/devices`, 장치의 노드, `/boot`, 그 디렉터리 중 하나, 또는 오버레이 파일이 존재하지만 읽을 수 없음 |
| `io.permission_denied` | 위와 같으나 `EACCES`로 실패함. 또는 `fw_printenv`가 존재하지만 실행할 수 없음(`EACCES` 또는 `EPERM`) |
| `peripherals.discovery_failed` | `fw_printenv`를 다른 이유로 시작할 수 없거나, 출력을 읽을 수 없거나, 종료 대기에 실패했거나, 제시간에 끝나지 않았거나, `dtbos`가 설정되지 않은 것 이외의 이유로 실패 종료함(이유에는 stderr에 출력한 첫 줄이 표시됨). 오버레이 파일의 형식이 잘못되었거나 1 MiB보다 큼. 목록이 위의 한도 중 하나에서 잘림 |

각 목록의 문제는 하나의 오류로 보고되며, 첫 번째 문제의 코드와 이유, 그리고 나머지 문제의
개수가 함께 표시됩니다. `supported_sensors`의 경우 `/boot` 또는 그 디렉터리 중 하나의 목록을
읽을 수 없거나 목록이 잘린 경우도 건너뛴 오버레이 파일과 함께 집계됩니다. `configured_cameras`의
경우 I2C 장치 목록이 잘린 경우도 읽을 수 없거나 잘린 장치와 함께 집계됩니다. `fw_printenv`가 없거나,
변수가 `not defined`라는 메시지와 함께 실패 종료하는 경우(`dtbos`가 설정되지 않았을 때
u-boot-tools가 이렇게 동작함) `overlays`는 오류 없이 생략됩니다. DevKit에 포함된 libubootenv는
설정되지 않은 변수에 대해 빈 값을 출력하므로, 이 경우 `overlays`는 빈 목록이 됩니다. 그 밖의
실패 시에는 마지막 목록이 유지되고 오류가 보고되며, 한 번도 성공한 적이 없으면 `overlays`는
생략됩니다. 디바이스 트리, I2C 장치 또는 `/boot`가 없는 보드는 오류 없이 빈 목록을
보고합니다.

## 제한 사항

- CSI-2 규칙은 구조적입니다. 자체 CSI-2 엔드포인트가 있고
  아래에 센서가 없는 장치는 카메라가 아니더라도 나열됩니다. 예를 들어
  HDMI-CSI-2 브리지나, 센서가 기술되지 않은 GMSL 디시리얼라이저가
  이에 해당합니다.
- I2C 버스에 센서를 추가하는 오버레이 중, 해당 버스의 레이블이나 경로가
  `i2c`로 시작하지 않는 오버레이는 인식되지 않습니다.
- `data_lanes`는 노드 순서상 첫 번째 엔드포인트에서 가져옵니다. 폭이 서로 다른
  여러 엔드포인트를 가진 센서는 그중 하나만 보고합니다.
- 엔드포인트에 `data-lanes`가 없는 센서는 CSI-2
  소스로 인식되지 않으므로 `configured_cameras`와 `supported_sensors`에서 누락됩니다.
- 자체 CSI-2 엔드포인트를 가진 보조 칩은 구성된
  카메라로 나열됩니다. 예: METOAK-DUAL 오버레이의 `Metoak,xc9080`. 이 칩을 센서로 하는
  `camera.mipi` 카메라가 없으므로 `camera_id`가 있는 경우는 없습니다.
- `/boot`와 U-Boot 환경은 감시하지 않습니다. `fw_setenv`로 한 변경이나 오버레이 파일의
  설치 또는 제거는 다음 스캔에 나타나며, 다음 스캔은 카메라 또는 사운드 장치 이벤트 후나
  새로 고침 시 실행됩니다.

## 예시

[`catalog-example.json`](catalog-example.json)에 있는 것과 같은
Modalix DevKit의 IMX477입니다. 모델, 디바이스 트리 경로,
I2C 장치 및 오버레이 이름은 Sentinel에서 캡처한 것이 아니라 DevKit에서 옮겨 적은 것이며,
이 블록은 합성 테스트 픽스처로 검증됩니다. 오버레이가 더 많이 설치된 보드는
`supported_sensors`에 더 많은 오버레이를 나열합니다.

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
