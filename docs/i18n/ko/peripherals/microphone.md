# 마이크

`microphone`은 하나의 ALSA 캡처 PCM입니다. USB Audio Class 마이크,
웹캠이나 헤드셋의 마이크, 온보드(플랫폼)
사운드 카드의 캡처 PCM, 또는 상위 장치가 없는 카드(가상 카드, 예:
`snd-aloop`)의 캡처 PCM이 해당됩니다. 재생 전용 카드와 재생 PCM은 보고되지 않습니다.
웹캠의 카메라는 별도의 `camera` 장치입니다.

- **프로바이더:** `microphone.alsa`
- **다음의 uevent 발생 시 재스캔:** `sound`

탐지는 텍스트와 링크만 읽습니다:
`/sys/class/sound`의 카드 및 캡처 PCM과 각 카드의 `id`, `device` 링크 및 USB 상위 장치,
`/proc/asound/cards`의 카드 이름 및 드라이버(해당 줄의
id가 카드의 sysfs id와 일치할 때만 사용), 각 카드의 `pcmMc/info` 및 USB 오디오의 경우
`streamM`, 그리고 `/dev/snd/by-path` 및 `/dev/snd/by-id`의 udev 링크입니다.
PCM이나 컨트롤 장치를 절대 열지 않으므로 애플리케이션에서 마이크를 빼앗거나
믹서를 변경할 수 없습니다. ALSA procfs(`CONFIG_SND_PROC_FS`)가
필요하며, `CONFIG_SND_VERBOSE_PROCFS`가 없으면 `pcmMc/info`만 누락됩니다.

## 식별 정보

`id`는 `microphone:alsa:<16 hex digits>` 형식이며,
`identity.stable_key`의 64비트 FNV-1a 해시입니다:

```text
sysfs:<card's sysfs device, relative to /sys>:pcm<M>c
sysfs:devices/platform/soc/xhci-hcd.0.auto/usb1/1-1.2/1-1.2:1.0:pcm0c
```

USB 마이크의 경우 sysfs 장치는 해당 포트의 오디오 인터페이스이므로
id는 같은 포트로의 재연결, 재부팅 및 ALSA 카드
번호 재지정 후에도 유지되며, 서로 다른 포트에 있는 동일한 마이크는 서로 다른 id를 갖습니다.
카드 번호, 카드 id 및 `/dev/snd` 이름은 id에 포함되지 않습니다.

상위 장치가 없는 카드(`/sys/devices/virtual/sound` 아래에 있으며 `device`
링크가 없음)는 장치 경로가 없으므로 키로 현재 카드 id를 사용합니다:
`alsa-card-id:<card id>:pcm<M>c`. 번호 재지정 후에도 유지되지만
카드 id가 바뀌면(드라이버의 `id` 옵션) 변경되며, 같은 드라이버의 두 카드는
부팅할 때마다 접미사가 붙은 id(`Loopback`, `Loopback_1`)가 서로 바뀔 수 있습니다. 이러한
레코드에는 `peripherals.sysfs_device_missing` 이슈가 포함됩니다. 현재
sysfs id가 없거나 비어 있는 카드는 이후 스캔까지 건너뜁니다. 읽을 수 없는
카드는 스캔을 실패시킵니다.

## 필드

| 필드 | 의미 |
| --- | --- |
| `type`, `id` | `microphone` 및 위의 id. |
| `name` | 카드의 짧은 이름, 없으면 PCM 이름, 그것도 없으면 카드 id. |
| `backend` | `alsa`. |
| `connection` | `usb`(카드의 장치에 USB 상위 장치가 있음), `platform` 또는 `unknown`(상위 장치 없음). |
| `capture_target` | `card_id`, `device`(PCM 번호), 그리고 카드 id가 문자, 숫자, `_` 및 `-`만 포함하고 한 자리 또는 두 자리 숫자가 아닌 경우(ALSA는 `CARD=7`을 카드 인덱스 7로 해석함) `selector`: `plughw:CARD=<card_id>,DEV=<M>`. 현재 부팅에서만 유효합니다. |
| `identity` | `stable_key`, `card_index`(재연결 시 변경됨), `pcm_node`(`/dev/snd/pcmC<N>D<M>c`), `card_id`; 알려진 경우 `card_name`, `card_driver`, `pcm_name`, `by_path` 및 `by_id`(카드의 `controlC<N>`을 가리키는 udev 링크 중 이름순으로 첫 번째), 그리고 USB의 경우 `usb`: `vendor_id`, `product_id`, `bus_path`(포트, 예: `1-1.2`) 및 존재하는 경우 `interface`, `manufacturer`, `product`, `serial`. |
| `modes` | `streamM`에 있는 각 캡처 altset의 형식마다 하나의 항목이며, 인터페이스와 altset 순으로 정렬되고 중복은 없습니다: `format`(예: `S16_LE`) 및 커널이 출력하는 경우 `interface`, `altset`, `channels`, `sample_bits`, `rates_hz`(정렬됨) 또는 `rate_range_hz`(`{"min", "max"}`), 그리고 `channel_map`(알 수 없는 위치는 `--`). 0이거나 역전된 값은 제외됩니다. 드라이버가 PCM을 열지 않고는 형식을 게시하지 않는 경우(USB가 아닌 모든 드라이버) 비어 있습니다. |
| `availability` | `state`: `available`, `in_use`(사용 가능한 캡처 서브디바이스 없음) 또는 `unknown`; 알려진 경우 `subdevices` 및 `subdevices_available` 포함. |
| `issues` | 읽을 수 없는 각 부분에 대한 `{"code", "reason"}`; 비어 있으면 생략됩니다. 레코드는 그래도 게시됩니다. |

이슈 코드: `peripherals.pcm_info_unreadable`,
`peripherals.capture_selector_unavailable`,
`peripherals.capabilities_unavailable`, `peripherals.availability_unknown`,
`peripherals.sysfs_device_missing`.

## 가용성은 스냅샷입니다

`availability`은 최근 스캔에서 ALSA가 보고한 상태입니다. Sentinel은
핫플러그와 새로 고침 시 다시 스캔하지만, 애플리케이션이 PCM을 열거나
닫는 시점은 알 수 없습니다. PulseAudio 같은 사운드 서버는 새로 연결된
마이크를 잠시 점유하므로, 핫플러그 직후의 스캔은 다음 새로 고침까지 `in_use`을
보고할 수 있습니다. 캡처하기 전에 실시간으로 확인하고(PCM을 열고 `EBUSY`를 처리) 어느
상태도 보장으로 간주하지 마십시오.

## 예시

```json
{"type": "microphone", "id": "microphone:alsa:a428cdcba66905a4",
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
 "availability": {"state": "available", "subdevices": 1, "subdevices_available": 1}}
```
