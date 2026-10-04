# 주변 장치

Sentinel은 Modalix DevKit에 연결된 장치의 카탈로그를 유지합니다. 어떤 장치가 있는지,
장치를 식별하는 방법과 장치의 기능을 기록합니다. Neat Core, Insight, sima-cli2,
스크립트와 에이전트가 모두 같은 카탈로그를 읽으므로 여기에서 추가한 장치 유형은 모든 곳에 동시에 표시됩니다.

카탈로그는 확장을 고려해 설계되었습니다. 카메라가 첫 번째 장치 유형이지만 다른 유형
(마이크, IMU, LiDAR 등)을 추가해도 카탈로그, API, CLI 또는 클라이언트를 변경할 필요가 없습니다.

## 작동 방식

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

**공급자**는 커널 인터페이스에서 한 장치 계열을 검색해 레코드를 반환하는 Sentinel 내부의
Rust 코드입니다. 핫플러그 깨우기, 디바운스, 오류 격리, 실패한 공급자의 마지막 정상 레코드 유지,
안정적인 리비전, 변경 로그, API와 CLI 등 나머지는 Sentinel이 처리합니다.

검색은 비용이 낮도록 설계되었습니다. 변경이 없을 때 스레드는 CPU를 사용하지 않고,
데몬보다 nice 수준을 10만큼 낮춰 실행하며, 연속된 이벤트와 새로 고침 요청을 한 번의 스캔으로 합칩니다.

## 페이지

| 페이지 | 대상 |
| --- | --- |
| [장치 유형 추가](adding-a-device-type.md) | 새로운 종류의 장치 지원을 추가하는 기여자 |
| [장치 유형](device-types/README.md) | [카메라](device-types/camera.md)부터 시작하는 지원 유형별 레코드 형식 |
| [로컬 에이전트 API](../api.md) | `GET /v1/peripherals`, `POST /v1/peripherals/refresh`, `since_revision` 폴링 |

## 카탈로그 사용

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

Neat 애플리케이션에서는 `simaai::neat::peripherals::list()`(C++)와
`pyneat.peripherals.list()`(Python)가 같은 카탈로그를 반환합니다. 각 장치는 세부 정보를 JSON
(`details_json` / `details`)으로 가지므로 Core가 형식화된 필드를 추가하기 전에도 Neat 애플리케이션에서
새 장치 유형을 사용할 수 있습니다.

<a id="support-rules"></a>

## 지원 규칙

각 카메라 모드의 `supported`와 `reason`은 설치된 Neat Core의 `CameraInput`이 해당 모드를
허용하는지 나타냅니다. 이 결정은 Sentinel이 아니라 Neat Core가 합니다. Neat Core는 규칙을
`/usr/share/simaai-sentinel/support/neat-core.json`에 설치하고, 주변 장치 스레드는 카탈로그를
비교하고 쓰기 전에 모든 모드에 적용합니다. 따라서 Core 업그레이드도 다른 변경처럼 `revision`을 증가시킵니다.
Sentinel은 디렉터리를 감시하고 하드웨어를 다시 스캔하지 않고 규칙을 다시 적용합니다. Neat Core가 없으면
모든 모드는 `supported: false`이며 이유에 Core가 설치되지 않았다고 표시됩니다.

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

규칙은 이 순서로 검사되며 첫 번째 실패가 모드의 `reason`이 됩니다. 크기 범위는 지원됨으로 표시되지 않습니다.
`isp_output`이 있으면 모드는 ISP 출력 크기여야 합니다. 카탈로그 최상위의 `support`는 `state`
(`applied`, `not_installed`, `invalid`, 또는 잘못된 업데이트 후 이전 규칙을 계속 쓰는 `stale`),
`source`, `path`를 보고합니다.

Sentinel은 `/usr/share/simaai-sentinel/support/`를 만들지만 파일을 설치하지 않습니다. 따라서
Sentinel과 Neat Core가 같은 경로를 소유하지 않으며 각각 독립적으로 설치, 업그레이드, 제거할 수 있습니다.
현재는 형식 1만 있습니다. 형식이 추가되어도 Sentinel은 이전 형식을 계속 읽습니다. Sentinel보다 새로운
형식의 규칙 파일은 이전 규칙을 계속 사용하게 하고 Sentinel 업데이트가 필요하다고 보고합니다.

## 데몬 옵션

| 옵션 | 기본값 | 의미 |
| --- | --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` | 카탈로그를 쓰고 `simaai-sentinel peripherals`가 읽는 위치 |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` | Neat Core 카메라 지원 규칙 |
| `--no-peripherals` | off | 주변 장치 검색 없이 데몬 실행 |
