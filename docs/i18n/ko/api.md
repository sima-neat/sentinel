# 로컬 에이전트 API

Sentinel 데몬은 로컬 Unix 소켓 `/run/simaai-sentinel/api.sock`을 통해 버전이 지정된 HTTP/JSON API를 제공합니다. TCP 포트에서 연결을 수신하지 않습니다. API와 터미널 UI는 동일한 캐시 및 잠긴 체크포인트 저장소를 사용하므로 에이전트가 시작한 추적 정보는 UI 및 CLI에서 즉시 확인할 수 있습니다.

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## 엔드포인트

| 방법 및 경로 | 목적 |
| --- | --- |
| `GET /v1/health` | 버전, 최신성, 샘플 및 메트릭 개수, 오류, 활성 추적 정보. |
| `GET /v1/cache` | 완료된 실시간 캐시 문서입니다. |
| `GET /v1/metrics` | 지표 정의, 단위, 설명 및 임계값. |
| `GET /v1/samples/latest` | 가장 최근의 시간 정보가 포함된 측정값입니다. |
| `GET /v1/traces/active` | 활성 추적 또는 `null`. |
| `POST /v1/traces` | 이름이 지정된 추적을 시작합니다. |
| `POST /v1/traces/stop` | 현재 추적을 중지하고 계속합니다. |
| `GET /v1/runs` | 현재 진행 중인 작업과 완료된 작업의 요약 목록을 표시합니다. |
| `GET /v1/runs/{name-or-id}` | 저장된 실행 기록과 원본 샘플을 불러옵니다. |
| `GET /v1/compare?runs=A,B` | 두 개 이상의 실행 결과를 비교합니다. 첫 번째 실행 결과는 기준선입니다. |

요청 시작 예시:

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -H 'Content-Type: application/json' \
  -d '{"name":"baseline","note":"before optimization","tags":["compiler-v1"]}' \
  http://localhost/v1/traces
```

활성 추적을 중지합니다.

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/stop
```

이름은 고유해야 하며, 활성화될 수 있는 추적은 하나뿐입니다. 충돌하는 라이프사이클 작업은 HTTP 409 오류를 반환합니다. 알 수 없는 실행 및 경로는 HTTP 404 오류를 반환합니다. 유효하지 않은 요청은 HTTP 400 오류를 반환합니다. 응답은 사용할 수 없는 지표에 대해 JSON `null` 형식을 사용합니다. 비교 응답에는 기본적으로 실행 메타데이터, 통계 및 기준선 차이가 포함됩니다. 타임스탬프가 지정된 샘플이 필요한 경우에만 `raw=1`을 추가합니다.

## 보안 및 동시성

이 소켓은 DevKit에 로컬로 존재하며, Sentinel에 의해 원격으로 노출되지 않습니다.

지원되는 제어 작업은 텔레메트리 추적을 시작하고 중지하는 것뿐이므로, 로컬 사용자가 접근할 수 있도록 의도적으로 설계되었습니다. API는 실행을 삭제하거나, 워크로드를 실행하거나, 하드웨어를 수정하지 않습니다. 원격 접근은 이 소켓을 포워딩하거나 인증되지 않은 TCP 리스너를 추가하는 대신, 인증된 Kerrigan/Fleet Manager 프록시를 통해 제공해야 합니다.

캐시 쓰기 작업은 원자적 이름 변경을 사용합니다. 실행 작업은 CLI, TUI 및 데몬 레코더와 동일한 배타적 파일 잠금을 사용합니다. 따라서 API를 통해 시작된 추적은 지원되는 모든 인터페이스를 통해 안전하게 검사하거나 중지할 수 있습니다.

## 에이전트 기술

설치 과정에서 생성된 Sentinel 설치 스크립트는 패키지가 빌드된 정확한 Sentinel Git 커밋을 사용하여 `sima-cli
playbooks install`을 실행합니다. 이렇게 하면 런타임 및 스킬 버전을 일치시키면서 Vulcan 아티팩트에 중첩된 스킬 리소스를 추가하지 않아도 됩니다. 플레이북 관리자는 각 지원되는 에이전트에 대해 스킬을 설치하고 로컬 레지스트리에 해당 소스 커밋을 기록합니다.

DevKit에서 Sentinel을 설치하는 경우 항상 `sima-cli neat install
sentinel`로 시작하므로 일반적으로 별도의 스킬 설치 단계가 필요하지 않습니다. DevKit 패키지가 설치되지 않은 환경(예: SDK 컨테이너 또는 개발자 워크스테이션)에서 Sentinel 스킬을 사용하려면 해당 환경의 `sima-cli`를 사용하여 GitHub에서 스킬을 직접 설치하십시오.

```bash
sima-cli playbooks install gh:sima-neat/sentinel/skills/use-sentinel
```

`main`에 적용되기 전에 변경 사항을 테스트하려면 다음 Git 참조를 추가하세요.

```bash
sima-cli playbooks install --force \
  gh:sima-neat/sentinel/skills/use-sentinel@feature/checkpoint-run-comparison
```

기술을 업데이트하거나 제거하는 작업은 계속해서 `sima-cli playbooks`에서 관리됩니다.
