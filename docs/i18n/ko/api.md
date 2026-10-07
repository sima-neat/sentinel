# 로컬 에이전트 API

Sentinel 데몬은 로컬 Unix
소켓 `/run/simaai-sentinel/api.sock`을 통해 버전이 지정된 HTTP/JSON API를 제공합니다. TCP 포트에서는 수신 대기하지 않습니다.
API와 터미널 UI는 동일한 캐시와 잠긴 체크포인트 저장소를 사용하므로, 에이전트가 시작한 추적은
UI와 CLI에서 즉시 확인할 수 있습니다.

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## 엔드포인트

| 메서드 및 경로 | 목적 |
| --- | --- |
| `GET /v1/health` | 버전, 최신성, 샘플 및 지표 개수, 오류, 활성 추적. |
| `GET /v1/cache` | 전체 실시간 캐시 문서. |
| `GET /v1/metrics` | 지표 정의, 단위, 설명 및 임계값. |
| `GET /v1/samples/latest` | 타임스탬프가 포함된 최신 지표 값. |
| `GET /v1/traces/active` | 활성 추적 또는 `null`. |
| `POST /v1/traces` | 이름이 지정된 추적을 시작합니다. |
| `POST /v1/traces/stop` | 활성 추적을 중지하고 저장합니다. |
| `POST /v1/traces/{id}/stop` | ID가 여전히 일치하는 경우에만 활성 추적을 중지합니다. |
| `GET /v1/runs` | 활성 및 완료된 실행의 요약 목록을 표시합니다. |
| `GET /v1/runs/{name-or-id}` | 저장된 실행과 원시 샘플을 조회합니다. |
| `GET /v1/compare?runs=A,B` | 두 개 이상의 실행을 비교합니다. 첫 번째 실행이 기준선입니다. |
| `GET /v1/peripherals` | 메모리에 보관된 연결된 주변 장치 목록. |
| `POST /v1/peripherals/refresh` | 다시 스캔한 후 새 카탈로그를 반환합니다. |

시작 요청 예시:

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -H 'Content-Type: application/json' \
  -d '{"name":"baseline","note":"before optimization","tags":["compiler-v1"]}' \
  http://localhost/v1/traces
```

활성 추적 중지:

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/stop
```

먼저 활성 추적을 읽은 클라이언트는 중지할 때 해당 ID를 포함해야 합니다:

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/20261004T120000.000Z-baseline/stop
```

다른 클라이언트가 추적을 대체한 경우 조건부 형식은 아무것도 중지하지 않고
HTTP 409를 반환합니다. ID 비교와 중지는 동일한 실행 저장소
잠금 아래에서 수행됩니다.

이름은 고유해야 하며, 한 번에 하나의 추적만 활성화될 수 있습니다. 충돌하는 수명 주기
작업은 HTTP 409를 반환합니다. 알 수 없는 실행 및 경로는 HTTP 404를 반환합니다. 유효하지 않은
요청은 HTTP 400을 반환합니다. 응답은 사용할 수 없는 지표에 JSON `null`을 사용합니다.
비교 응답에는 기본적으로 실행 메타데이터, 통계 및 기준선 대비 차이가
포함됩니다. 타임스탬프가 지정된 샘플이 필요한 경우에만 `raw=1`을 추가하십시오.

## 주변 장치

탐지 스레드는 데몬이 시작될 때,
커널이 장치 변경을 보고할 때, 그리고 새로 고침 요청이 있을 때 보드의 주변 장치를 스캔합니다. 장치
프로바이더는 커널 인터페이스만 읽으며 스트림을 열지 않습니다. `board` 블록의 U-Boot 오버레이 목록을
읽기 위해 각 스캔은 `fw_printenv -n dtbos`도 root로 실행합니다. 이 도구는 환경을 변경하지 않지만
환경의 잠금을 획득하므로 `/var/lock/fw_printenv.lock`이 생성됩니다. `GET /v1/peripherals`는
최신 결과를 반환합니다:

```json
{"revision": 1791155282460, "observed_at": "2026-10-04T23:08:02.460Z",
 "board": {"model": "SiMa.ai Modalix SoM 16Gig Board", "configured_cameras": [...], ...},
 "devices": [{"type": "camera", "id": "camera:v4l2:3f2a9c0d41b7e650", ...}],
 "errors": [{"provider": "camera.v4l2", "code": "io.permission_denied", "reason": "..."}]}
```

| 필드 | 의미 |
| --- | --- |
| `revision` | `board`, `devices` 또는 `errors`가 변경될 때마다 바뀝니다. 데몬이 시작될 때 임의의 값에서 시작하므로 재시작이나 시계 변경으로 같은 값이 반복될 가능성은 매우 낮습니다. 같은지 여부만 비교하십시오. 항상 2^52 미만이므로 double을 사용하는 JSON 리더에서도 정확하게 표현됩니다. |
| `observed_at` | 이 결과의 기반이 된 스캔이 시작된 시각. 첫 스캔이 완료될 때까지는 `null`입니다. |
| `board` | 보드가 카메라용으로 설정된 방식: 모델, U-Boot가 적용하는 오버레이, 디바이스 트리가 구성하는 카메라(카탈로그에 해당 카메라가 있으면 각각 자신의 센서에 해당하는 `camera.mipi` 카메라의 `id` 포함), 설치된 오버레이가 지원하는 센서. `devices`와 같은 스캔에서 읽으며, 첫 스캔이 완료될 때까지는 없습니다. [보드 카메라 구성](peripherals/board.md)을 참조하십시오. |
| `devices` | 장치당 하나의 객체이며 `type`으로 구분됩니다. `id`는 같은 포트에 다시 연결해도 유지되며 `/dev/videoN` 이름이 되는 일은 없습니다. |
| `errors` | 최근 스캔에서 실패한 프로바이더, 그리고 `board` 블록 중 완전히 읽을 수 없었던 필드(프로바이더 `board.<field>`, 예: `board.overlays`). 실패한 프로바이더가 마지막으로 성공한 스캔에서 찾은 장치는 `devices`에 그대로 남으며, 실패한 `fw_printenv`는 마지막 오버레이 목록을 유지합니다. `hotplug.unavailable`은 커널 uevent를 수신할 수 없어 새로 고침 시에만 재스캔이 이루어진다는 의미입니다. |

Sentinel은 하드웨어 사실만 보고합니다. 애플리케이션이
장치나 모드를 지원하는지는 해당 애플리케이션이 결정합니다. `CameraInput`의 경우
Neat Core가 이를 결정합니다.

`POST /v1/peripherals/refresh`는 요청 이후에 시작되는 스캔을 기다린 뒤
해당 카탈로그와 함께 HTTP 200을 반환하며, 이는 `GET`과 동일한 문서입니다. 동시에 들어온
새로 고침 요청은 스캔을 공유합니다. 스캔이 10초 이내에 완료되지 않으면
HTTP 504를, 이미 8개의 새로 고침 요청이 대기 중이면 HTTP 429를 반환합니다. 두
경로 모두 탐지가 비활성화되었거나(`--no-peripherals`)
중지된 경우 HTTP 503을 반환합니다.

`simaai-sentinel peripherals`는 같은 결과를 표로 출력합니다. 원시 문서를 보려면 `--json`을,
먼저 다시 스캔하려면 `--refresh`를 추가하십시오.

장치 레코드는 유형별로 설명되어 있습니다: [USB 카메라](peripherals/camera.md),
[MIPI CSI-2 카메라](peripherals/camera-mipi.md),
[마이크](peripherals/microphone.md). `board` 블록은
[보드 카메라 구성](peripherals/board.md)에 설명되어 있습니다.

[`peripherals/catalog-example.json`](peripherals/catalog-example.json)은
DevKit에서 얻은 전체 응답이며, USB 카메라는 형식당 하나의 모드로
축약되어 있습니다. 이 응답의 `board` 블록은 캡처한 것이 아니라 DevKit에서
옮겨 적은 것입니다. Sentinel 테스트가 이를 스키마와 대조하여 검사하므로 클라이언트도 이를 기준으로
테스트할 수 있습니다.

## 보안 및 동시성

이 소켓은 DevKit에 로컬로 존재하며 Sentinel에 의해 원격으로 노출되지 않습니다.
지원되는 제어 작업은 텔레메트리 추적을 시작하고 중지하는 것뿐이므로 로컬 사용자가
접근할 수 있도록 의도적으로 설계되었습니다. API는 실행을 삭제하거나,
워크로드를 실행하거나, 하드웨어를 수정하지 않습니다. 원격 접근은 이 소켓을 포워딩하거나
인증되지 않은 TCP 리스너를 추가하는 대신, 인증된 Kerrigan/Fleet Manager
프록시를 통해 제공해야 합니다.

캐시 쓰기는 원자적 이름 변경을 사용합니다. 실행 작업은 CLI, TUI 및 데몬 레코더와 동일한
배타적 파일 잠금을 사용합니다. 클라이언트가 관찰한 추적을 대체한 추적을 중지하면 안 되는 경우
ID를 포함하는 중지 경로를 사용하십시오.

## 에이전트 스킬

설치 중에 생성된 Sentinel 설치 스크립트는 패키지가 빌드된 정확한 Sentinel Git 커밋을 사용하여 `sima-cli
playbooks install`을 실행합니다.
이렇게 하면 Vulcan 아티팩트에 중첩된 스킬 리소스를 추가하지 않고도
런타임과 스킬 리비전을 일치시킬 수 있습니다. 플레이북 관리자는 지원되는 각 에이전트에 대해
스킬을 설치하고 해당 소스 커밋을 로컬
레지스트리에 기록합니다.

DevKit에서의 Sentinel 설치는 항상 `sima-cli neat install
sentinel`로 시작하므로 일반적으로 별도의 스킬 설치 단계가 필요하지 않습니다.
DevKit 패키지가 설치되지 않은 환경(예: SDK 컨테이너 또는 개발자 워크스테이션)에서
Sentinel 스킬을 사용하려면
해당 환경의 `sima-cli`를 사용하여 GitHub에서 스킬을 직접 설치하십시오:

```bash
sima-cli playbooks install gh:sima-neat/sentinel/skills/use-sentinel
```

`main`에 반영되기 전의 리비전을 테스트하려면 Git ref를 덧붙이십시오:

```bash
sima-cli playbooks install --force \
  gh:sima-neat/sentinel/skills/use-sentinel@feature/checkpoint-run-comparison
```

스킬 업데이트 및 제거는 계속 `sima-cli playbooks`에서 관리합니다.
