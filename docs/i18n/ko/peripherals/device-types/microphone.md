# 마이크

ALSA가 노출하는 오디오 캡처 장치: USB 오디오 클래스 마이크,
웹캠이나 헤드셋과 같은 복합 USB 장치의 마이크, 캡처
온보드(플랫폼) 사운드 카드의 PCM 및 사운드 카드가 없는 카드의 PCM을 캡처합니다.
상위 장치(가상 카드). 각 캡처 PCM은 하나의 레코드입니다.
재생 전용 장치 및 헤드셋의 재생 측면은 보고되지 않으며,
웹캠 카메라는 카메라 제공업체에서 별도로 보고합니다.

- **토큰 유형:** `microphone`
- **공급자:** `daemon.audio.alsa`(내장)
- **재검색 트리거:** `sound`

검색은 커널 텍스트 및 sysfs만 읽습니다. 선택 사항 `/proc/asound/cards`,
선택 사항 `/proc/asound/cardN/pcmMc/info`,
`/proc/asound/cardN/streamM`(USB 오디오), `/sys/class/sound/pcmCNDMc`,
`/sys/class/sound/cardN/device` 및 해당 USB 조상과 udev 링크
`/dev/snd/by-path` 및 `/dev/snd/by-id`. PCM이나 컨트롤이 열리지 않습니다.
장치이므로 애플리케이션에서 마이크를 가져오거나 변경할 수 없습니다.
믹서. `CONFIG_SND_PROC_FS` 없이 빌드된 커널은 `/proc/asound`를 모두 생략합니다.
Sentinel은 카드를 열거하고 sysfs에서 PCM을 캡처하고 sysfs 카드를 유지합니다.
ID 및 캡처 선택기를 사용하며 procfs 전용 이름, 드라이버, 모드 및
해당 문제가 있는 가용성 메타데이터. 없이 구축된 커널
`CONFIG_SND_VERBOSE_PROCFS`는 `pcmMc/info`만 생략합니다. sysfs 클래스 장치
여전히 가용성을 알 수 없는 레코드를 생성하며
`peripherals.pcm_info_unreadable` 문제.

## 신원

`id`는 64비트 FNV-1a 해시인 `microphone:alsa:<16 hex digits>`입니다.
`identity.stable_key`:

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

USB 장치의 경우 sysfs 장치는 해당 포트의 오디오 제어 인터페이스입니다.
따라서 동일한 포트에 다시 연결하고 재부팅하고 ALSA를 수행해도 ID는 동일하게 유지됩니다.
카드 번호를 다시 매기면 다른 포트에 있는 두 개의 동일한 마이크가
다른 아이디. 다른 포트는 다른 마이크입니다. 카드번호,
카드 ID 및 `/dev/snd` 노드 이름은 라우팅 세부 정보이므로 ID를 입력하지 마세요.
장치 경로가 있는 카드입니다.
키와 해시는 Neat Core의 이전 ALSA 공급자가 사용한 것입니다.
마이크는 ID를 유지합니다.

상위장치(가상카드, 드라이버 등) 없이 등록된 카드
`snd_card_new`에 상위 항목을 전달하지 않음)는 `/sys/devices/virtual/sound` 아래에 있습니다.
`device` 링크가 없으므로 키를 켤 장치 경로가 없습니다. 그 핵심은
대신 카드 ID:

```text
alsa-card-id:<card id>:pcm<M>c
alsa-card-id:Loopback:pcm0c
```

카드 ID는 사용 가능한 가장 좋은 속성입니다. 카드 번호를 다시 매겨도 유지됩니다.
재부팅하고 ALSA는 존재하는 카드 중에서 이를 고유하게 유지하므로 이러한 두 가지
카드는 키를 공유하지 않습니다. 제한 사항: 카드 ID 변경(드라이버의 `id`
모듈 옵션을 사용하거나 `/sys/class/sound/cardN/id`)를 작성하면 레코드 ID가 변경됩니다.
한 드라이버의 두 카드가 모두 존재할 경우 커널은 다음을 접미사로 붙입니다.
등록 순서에 있는 두 번째 ID(`Loopback_1`)를 사용하여 ID를 교환할 수 있습니다.
부츠 사이. 커널은 빈 ID를 가진 카드를 등록하지 않습니다. 실시간 sysfs ID가
비어 있거나 읽을 수 없으면 Sentinel은 스냅샷을 일시적인 것으로 간주하고
나중에 다시 검색할 때까지 카드를 건너뜁니다.

## 세부

| 필드 | 유형 | 항상 존재 | 의미 | 소스 |
| --- | --- | --- | --- | --- |
| `name` | 문자열 | 예 | 카드 짧은 이름, 그렇지 않으면 PCM 이름, 그렇지 않으면 카드 ID, 그렇지 `ALSA capture PCM <M>` | `/proc/asound/cards`, `pcmMc/info` |
| `backend` | 문자열 | 예 | `alsa` | |
| `connection` | 문자열 | 예 | `usb` 카드의 장치에 USB 조상이 있는 경우, `unknown` 카드에 상위 장치가 없는 경우, 그렇지 않으면 `platform` | sysfs |
| `capture_target` | 개체 | 예 | `card_id`(비어 있지 않은 문자열), `device`(PCM 번호) 및 `selector`(`plughw:CARD=<card_id>,DEV=<M>`)인 경우 카드 ID에는 문자, 숫자, `_` 및 `-`만 포함되며 한 자리 또는 두 자리 숫자가 아닙니다(ALSA는 `CARD=7`을 카드 인덱스 7로 읽습니다). 현재 부팅 전용 라우팅 | `/sys/class/sound/cardN/id` |
| `identity` | 개체 | 예 | 아래 참조 | |
| `modes` | 정렬 | 예 | 캡처 형식; 드라이버가 아무것도 게시하지 않으면 비어 있습니다(참조 `issues`) | `streamM` |
| `availability` | 물체 | 예 | `state`: `available`, `in_use` (캡처 하위 장치가 비어 있지 않음) 또는 `unknown`; ~와 함께 `subdevices` 그리고 `subdevices_available` 알려졌을 때. 마지막 스캔의 스냅샷 보다 [유효성](#availability) | `pcmMc/info` |
| `issues` | 배열 | 아니요 | 읽을 수 없는 각 부분에 대한 `{"code", "reason"}`; 기록이 여전히 게시되어 있습니다. | |

### `identity`

| 필드 | 유형 | 항상 존재 | 의미 |
| --- | --- | --- | --- |
| `stable_key` | 문자열 | 예 | 키 `id` 해시(위) |
| `card_index` | 정수 | 예 | 현재 ALSA 카드 번호(다시 꽂을 때마다 변경됨) |
| `pcm_node` | 문자열 | 예 | `/dev/snd/pcmC<N>D<M>c`(라우팅 전용) |
| `card_id`, `card_name`, `card_driver` | 문자열 | `card_id`는 항상 존재하며 나머지는 비어 있지 않은 경우 | ID는 실시간 sysfs에서 가져옵니다. 이름과 드라이버는 `/proc/asound/cards`에서 나옵니다. 예: `Nano`, `Yeti Nano`, `USB-Audio` |
| `pcm_name` | 문자열 | 비어 있지 않은 경우 | PCM 이름, 예: `USB Audio` |
| `by_path`, `by_id` | 문자열 |(udev 포함) | 이름 순서대로 카드의 `controlC<N>`에 대한 첫 번째 `/dev/snd/by-path` / `/dev/snd/by-id` 링크 |
| `usb` | 개체 | USB 전용 | `vendor_id`, `product_id`, `bus_path`(USB 포트, 예: `1-1.2`) 및 존재하는 경우 `interface` (예: `1-1.2:1.0`), `manufacturer`, `product`, `serial` |

### 모드

USB 오디오 `streamM` 파일에 있는 각 캡처 대체 세트의 형식당 하나의 모드입니다.
모드가 정렬되고 중복 항목이 제거됩니다.

| 필드 | 유형 | 존재 | 의미 |
| --- | --- | --- | --- |
| `format` | 문자열 | 항상 | ALSA 샘플 형식, 예: `S16_LE`, `S24_3LE`, `S32_LE` |
| `interface`, `altset` | 정수 | 인쇄 시 | USB 인터페이스 및 대체 설정 |
| `channels` | 정수 | 양수인 경우 | 채널 수 |
| `sample_bits` | 정수 | 양수인 경우 | 샘플당 유효한 비트 |
| `rates_hz` | 정수 배열 | 이산 요율 | 중복이나 0 없이 정렬됨 |
| `rate_range_hz` | 객체 | 연속 속도 | `{"min", "max"}`; 모드에는 `rates_hz` 또는 `rate_range_hz` 중 하나만 존재합니다. |
| `channel_map` | 문자열 배열 | 인쇄 시 | 채널 위치, 예: `["FL", "FR"]`, `["MONO"]`; 알 수 없는 위치에 대한 `--` |

### 유효성

`availability`는 ALSA가 마지막 스캔에서 보고한 내용이며 라이브 상태가 아닙니다.
Sentinel은 핫 플러그 이벤트 및 새로 고침 요청을 다시 검색하지만 아무 것도 알려주지 않습니다.
응용 프로그램이 PCM을 열거나 닫을 때 발생합니다. 다음과 같은 사운드 서버
PulseAudio는 새로 연결된 마이크를 잠시 엽니다(C920의 경우 5~8초).
DevKit), 핫플러그 직후의 스캔에서 `in_use`를 보고할 수 있으며 기록
다음 새로 고침 또는 핫 플러그까지 `in_use`를 유지합니다. 고객은 실시간으로 확인해야 합니다.
캡처하기 전에(예: PCM을 열고 `EBUSY` 처리)
먼저 새로 고치고 `in_use` 또는 `available`을 보장으로 처리하면 안 됩니다.

### 문제 코드

| 코드 | 조건 |
| --- | --- |
| `peripherals.pcm_info_unreadable` | `pcmMc/info`를 읽을 수 없습니다. `CONFIG_SND_VERBOSE_PROCFS`가 비활성화되었습니다. |
| `peripherals.capture_selector_unavailable` | 카드 ID로 안전한 `selector`를 구성할 수 없거나, ALSA가 카드 인덱스로 읽는 한 자리 또는 두 자리 숫자입니다. |
| `peripherals.capabilities_unavailable` | 캡처 형식 없음: USB가 아닌 드라이버(형식은 PCM을 열지 않고 USB 오디오로만 게시됨) 또는 드라이버가 없는 USB 스트림 |
| `peripherals.availability_unknown` | `subdevices_count` / `subdevices_avail` 누락 또는 유효하지 않음 |
| `peripherals.sysfs_device_missing` | 카드에는 sysfs에 상위 장치가 없습니다. 버스나 USB ID가 없으며 ID는 카드 ID를 따릅니다(참조). [신원](#identity)) |

## 예시 레코드

```json
{"id": "microphone:alsa:a428cdcba66905a4", "type": "microphone", "provider": "daemon.audio.alsa",
 "microphone": {
   "name": "Yeti Nano", "backend": "alsa", "connection": "usb",
   "capture_target": {"card_id": "Nano", "device": 0, "selector": "plughw:CARD=Nano,DEV=0"},
   "identity": {"stable_key": "sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c",
                "card_index": 0, "pcm_node": "/dev/snd/pcmC0D0c", "card_id": "Nano",
                "card_name": "Yeti Nano", "card_driver": "USB-Audio", "pcm_name": "USB Audio",
                "by_id": "/dev/snd/by-id/usb-Blue_Microphones_Yeti_Nano_REV8-00",
                "usb": {"vendor_id": "b58e", "product_id": "0005", "bus_path": "1-1.2",
                        "interface": "1-1.2:1.0", "manufacturer": "Blue Microphones",
                        "product": "Yeti Nano", "serial": "REV8"}},
   "modes": [{"format": "S16_LE", "interface": 2, "altset": 1, "channels": 2, "sample_bits": 16,
              "rates_hz": [48000], "channel_map": ["FL", "FR"]},
             {"format": "S24_3LE", "interface": 2, "altset": 2, "channels": 2, "sample_bits": 24,
              "rates_hz": [48000], "channel_map": ["FL", "FR"]}],
   "availability": {"state": "available", "subdevices": 1, "subdevices_available": 1}}}
```

## 변형 적용

- 모노, 스테레오 및 다중 채널; 16, 24, 32비트 형식; 여러 형식
  하나의 대체 세트에서; 여러 Altset 및 인터페이스.
- 개별 요금 목록(정렬, 중복 제거) 및 연속 범위 0 또는
  반전된 값은 게시되지 않고 삭제됩니다.
- 재생 전용 카드(녹음 없음) 및 헤드셋(캡처 전용) 웹캠
  마이크(카메라 옆에 있는 자체 기록).
- 서로 다른 포트에 여러 개의 동일한 마이크가 있습니다. 여러 개의 캡처 PCM
  카드 한 장; 다시 꽂을 때마다 변경되는 카드 번호.
- 제조업체, 제품 또는 일련번호가 누락되었습니다(생략). udev 링크 누락;
  선택기에서 안전하지 않거나 ALSA가 카드로 읽는 카드 ID
  색인; 읽을 수 없음 `pcmMc/info`;
  공개된 형식이 없는 비 USB 카드; 녹음 중인 스트림
  (실행 상태 줄을 건너뜁니다. 가용성 `in_use`).
- ALSA가 없거나 사운드 카드가 없는 커널: 기록이 없습니다.
- `CONFIG_SND_VERBOSE_PROCFS`가 없는 커널: 캡처 PCM이 열거됩니다.
  sysfs에서 제공되며 가용성을 알 수 없는 상태로 게시되었습니다.
- 상위 장치가 없는 카드(`/sys/devices/virtual/sound` 아래, 없음
  `device` 링크): 해당 캡처 PCM은 `connection: unknown`으로 나열됩니다.
  USB ID 없음 및 `peripherals.sysfs_device_missing` 문제 옆에
  다른 마이크.
- 추가 또는 제거 중인 카드(카드 목록 및 카드 디렉토리)
  동의하지 않음), `device` 링크가 누락된 상위 장치가 있는 카드
  (카드를 제거하는 동안에만 표시됨) 또는 해결할 수 없는 경우
  `idVendor` 및 `idProduct` 중 하나만 포함된 USB 조상이 실패합니다.
  스캔하므로 카탈로그는 마지막으로 좋은 기록을 유지하고 공급자에게 보고합니다.
  다음 스캔까지 발행합니다. sysfs 항목이나 장치가 사라진 카드
  대신 스캔하는 동안(플러그를 뽑고 경주하는 것)은 건너뜁니다.

## 지원 규칙

없음. 마이크 레코드에는 `supported` 또는 `reason`이 없습니다.

## 확인

| 동작 | 실제 하드웨어(어떤 장치) | 설비만 |
| --- | --- | --- |
| 웹캠 마이크: 카메라 옆에 레코드 1개, `connection: usb`, `plughw:CARD=C920,DEV=0`, S16_LE 16/24/32kHz에서 2채널(altsets 1-3), `by_id` 링크, 문제 없음 카메라는 영향을 받지 않음 | Logitech C920 내장 마이크, DevKit, 2026-10-04 | 합성 C920 유사 고정 장치 |
| 분리했다가 다시 연결한 후 동일 `id`(USB `authorized` 0, 1): 제거한 후 다시 추가 | Logitech C920, DevKit, 2026-10-04 | 카드 번호 다시 매기기: 합성 |
| `availability`는 `in_use`이고 `arecord` 레코드는 `available`입니다. 6초 동안 20번 새로 고쳐도 방해가 되지 않았습니다. | Logitech C920, DevKit, 2026-10-04 | |
| `availability` 핫 플러그가 유지된 후 `in_use` 동안 PulseAudio가 새 장치를 유지하는 동안 다음 스캔까지([유효성](#availability) 참조) | 로지텍 C920, DevKit, 2026-10-04 | |
| 스테레오 24비트, 모노, 32비트, 연속 속도, 대체 세트당 여러 형식, 헤드셋, 동일한 마이크, 플랫폼 카드, 상위 장치가 없는 카드, 부분 스냅샷, USB 문자열 누락 | 아직 없음 | 합성 `/proc/asound` 커널 6.18.3 레이아웃의 텍스트(`sound/core/init.c`, `sound/core/pcm.c`, `sound/usb/proc.c`) 및 합성 sysfs; Yeti Nano와 유사한 모노 및 플랫폼 고정 장치는 실제 장치를 캡처하지 않습니다. |
