# 주변기기

Sentinel은 Modalix DevKit에 연결된 장치의 카탈로그를 유지합니다.
존재하는 장치, 식별 방법 및 수행할 수 있는 작업에 대해 설명합니다. 깔끔한 코어,
Insight, sima-cli2, 스크립트 및 에이전트는 모두 동일한 카탈로그를 읽으므로 장치는
여기에 추가된 유형은 모든 곳에서 동시에 표시됩니다.

카탈로그는 성장을 위해 만들어졌습니다. 카메라와 마이크가 내장되어 있습니다.
장치 유형; 다른 유형(IMU, LiDAR 등)을 추가하는 데는 필요하지 않습니다.
카탈로그, API, CLI 또는 클라이언트가 변경됩니다.

## 작동 원리

```text
kernel hot-plug event ─┐
refresh request ───────┤
                       ▼
            peripherals thread (one per daemon, sleeps until woken)
              1. run every provider (read-only)
              2. apply Neat Core's support rules (cameras)
              3. compare with the last catalog; revision +1 if changed
              4. write /run/simaai-sentinel/peripherals.json
                       │
                       ▼
            GET /v1/peripherals  ·  simaai-sentinel peripherals
```

**공급자**는 Sentinel 내의 Rust 코드로, 하나의 계열을 발견합니다.
커널 인터페이스의 장치를 검색하고 레코드를 반환합니다. Sentinel이 모든 작업을 수행합니다.
else: 핫플러그에서 깨우기, 디바운싱, 오류 격리,
실패한 공급자의 마지막 양호한 기록 유지, 안정적인 개정, 변경
로그, API 및 CLI.

발견은 설계상 저렴합니다. 스레드는 CPU를 사용하지 않고 아무것도 변경되지 않습니다.
데몬 아래에서 10개의 멋진 레벨을 실행하고 일련의 이벤트를 병합하고 새로 고칩니다.
요청을 한 번에 스캔합니다.

## 페이지

| 페이지 | 용도 |
| --- | --- |
| [장치 유형 추가](adding-a-device-type.md) | 새로운 종류의 장치에 대한 지원을 추가하는 기여자 |
| [장치 유형](device-types/README.md) | 지원되는 각 유형의 레코드 형식: [카메라](device-types/camera.md), [마이크](device-types/microphone.md) |
| [로컬 에이전트 API](../api.md) | `GET /v1/peripherals`, `POST /v1/peripherals/refresh`, `since_revision`을 사용한 폴링 |

## 카탈로그 사용

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

Neat 애플리케이션에서 `simaai::neat::peripherals::list()`(C++) 및
`pyneat.peripherals.list()` (Python)는 동일한 카탈로그를 반환합니다. 모든 장치
세부 정보를 JSON (`details_json` / `details`)으로 전달하므로 새로운 장치 유형
Core가 입력된 필드를 추가하기 전에 Neat 애플리케이션에서 사용할 수 있습니다.

<a id="support-rules"></a>

## 지원 규칙

각 카메라 모드는 `supported` 및 `reason`을 전달합니다. Neat가 설치되었는지 여부
Core의 `CameraInput`이 이를 수락합니다. Sentinel은 이를 결정하지 않습니다. 깔끔한 코어
`/usr/share/simaai-sentinel/support/neat-core.json`에 규칙을 설치합니다.
주변 장치 스레드는 카탈로그가 작성되기 전에 이를 모든 모드에 적용합니다.
비교 및 작성되므로 코어 업그레이드는 다른 변경 사항과 마찬가지로 `revision`을 범프합니다.
Sentinel은 디렉터리를 감시하고 다시 검색하지 않고 규칙을 다시 적용합니다.
하드웨어. Neat Core가 없으면 모든 모드는 이유와 함께 `supported: false`입니다.
Core가 설치되지 않았다고 합니다.

```json
{
  "format": 1,
  "source": "neat-core 0.4.0",
  "camera": {
    "backends": {"accept": ["mipi"], "reason": "..."},
    "formats": {"accept": ["NV12"], "reason": "..."},
    "framerates": {"accept": [{"num": 30, "den": 1}], "reason": "..."},
    "isp_output": {"reason": "..."}
  }
}
```

규칙은 해당 순서대로 확인되며 첫 번째 실패는 모드의 실패가 됩니다.
`reason`. 크기 범위는 지원되는 것으로 표시되지 않습니다. `isp_output`이 있는 경우,
모드는 ISP 출력 크기여야 합니다. 카탈로그의 최상위 수준 `support`
`state` (`applied`, `not_installed`, `invalid` 또는 `stale` 보고 언제
잘못된 업데이트로 인해 이전 규칙이 사용 중임), `source` 및 `path`.

Sentinel은 `/usr/share/simaai-sentinel/support/`를 생성하지만 절대 설치하지 않습니다.
따라서 Sentinel과 Neat Core는 동일한 경로를 요구하지 않고 설치하지 않습니다.
독립적으로 업그레이드하거나 제거합니다. 형식 1은 지금까지 유일한 형식입니다. 언제
형식이 추가되면 Sentinel은 이전 형식을 계속 읽습니다. 규칙 파일은
Sentinel보다 최신 형식은 이전 규칙을 계속 사용하고 보고한다는 것을 알고 있습니다.
Sentinel에 업데이트가 필요합니다.

## 데몬 옵션

| 옵션 | 기본값 | 의미 |
| --- | --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` | 카탈로그를 쓰고 `simaai-sentinel peripherals`에서 읽는 위치 |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` | Neat Core의 카메라 지원 규칙 |
| `--no-peripherals` | off | 주변 장치 검색 없이 데몬 실행 |
