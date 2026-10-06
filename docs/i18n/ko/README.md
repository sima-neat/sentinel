# Sentinel 설명서

Sentinel은 백그라운드 데몬에서 Modalix DevKit 텔레메트리를 수집하고,
원자적 JSON 캐시를 작성한 다음, 해당 캐시를 터미널 보고서로 렌더링합니다. 이 페이지에서는
각 패널이 보고하는 내용, 각 값의 출처, 계산 방법,
그리고 제한 사항을 설명합니다.

## 운영 현황 패널

| 패널 | 문서 | 목적 |
| --- | --- | --- |
| 개요 | [개요 패널](panels/overview.md) | 주요 온도, 전력, CPU, 메모리, MLA 메모리, 저장 장치 및 네트워크 상태. |
| 열 | [열 패널](panels/thermal.md) | 칩 내부 RTSN/PVT 센서와 보드 수준 하드웨어 모니터 측정값. |
| 전력 | [전력 패널](panels/power.md) | PMBus 레일 전력, 합계, 세션 평균/최댓값 및 수집기 상태. |
| 시스템 | [시스템 패널](panels/system.md) | 전체/코어별 CPU, 평균 부하, Linux 메모리, MLA 할당, EV74 CMA 메모리 및 프로세스. |
| 저장 장치/네트워크 | [저장 장치 및 네트워크 패널](panels/storage-network.md) | eMMC/NVMe 용량 및 I/O와 전체 네트워크 트래픽. |

## 보고서 및 수집 동작

- [측정 모델](measurement-model.md)은 수집기 주기, 캐시
  기록, 차트 의미, 임계값, 오래된 데이터, 사용할 수 없는 값 및
  재설정 동작을 설명합니다.
- [보고서 및 JSON 내보내기](reports.md)는 캐시 스키마와
  `table`, `export`, `sensors` 및 `status` 명령을 설명합니다.
- [로컬 에이전트 API](api.md)는 데몬의
  Unix 소켓을 통한 실시간 읽기와 추적 제어를 설명합니다.
- [실행 캡처 및 비교](run-comparison.md)는 영구
  체크포인트, Compare Runs 탭, 보존 정책 및 CSV/JSON 내보내기를 설명합니다.

## 주변 장치

Sentinel은 보드에 연결된 카메라와 마이크도 나열합니다.
[로컬 에이전트 API](api.md#peripherals)에서 카탈로그와 이를 새로 고치는 방법을
설명하며, 각 장치 유형에는 별도의 페이지가 있습니다:

- [USB 카메라](peripherals/camera.md) (V4L2)
- [MIPI CSI-2 카메라](peripherals/camera-mipi.md)
- [마이크](peripherals/microphone.md) (ALSA)
- [보드 카메라 구성](peripherals/board.md): 모델, 카메라
  오버레이, 구성된 센서와 지원되는 센서
- 기여자를 위한 [장치 유형 추가](peripherals/adding-a-device-type.md)
