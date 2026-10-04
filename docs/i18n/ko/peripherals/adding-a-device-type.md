# 장치 유형 추가

이 페이지는 Sentinel이 새로운 종류의 장치를 검색하도록 만들려는 기여자를 위한 안내서입니다.
커널 인터페이스 확인, 공급자 작성과 테스트, 애플리케이션 개발자가 새 유형을 사용할 수 있도록 문서화하는 방법을 다룹니다.

## 1. 커널이 장치를 설명하는지 확인

모든 공급자는 sysfs, `/proc`, uevent, 읽기 전용 ioctl(V4L2, ALSA, IIO, media controller 등)의
커널 인터페이스를 읽는 Sentinel 내부 Rust 코드입니다. Sentinel은 장치 검색을 위해 다른 프로그램을 실행하거나
libcamera 또는 GStreamer 같은 사용자 공간 스택을 로드하지 않습니다. 시작하기 전에 커널이 애플리케이션에 필요한
장치 정보를 제공하는지 확인하십시오.

카메라 공급자가 동작 예입니다. USB 카메라는 `src/peripherals/v4l2/`, MIPI 카메라는
`src/peripherals/mipi/`에 있습니다.

## 2. 모든 공급자가 따르는 규칙

1. **읽기 전용.** 장치를 조회할 뿐 설정, 스트리밍, 소유권 획득 또는 커널 상태 변경을 하지 않습니다.
   장치 노드는 `O_RDONLY | O_NONBLOCK`으로 엽니다. 실행 중인 애플리케이션이 검색을 감지해서는 안 됩니다.
2. **안정적인 식별자.** 레코드의 `id`는 재연결, 재부팅, 번호 변경 후에도 같아야 합니다. 버스 토폴로지,
   일련번호, 커널 엔터티 이름 같은 안정적인 속성으로 만들고 `/dev/videoN`, 카드 번호, 열거 순서는 사용하지 않습니다.
   `camera:...`, `microphone:...`처럼 유형을 접두사로 붙입니다.
3. **한 표본이 아니라 전체 클래스를 지원합니다.** 책상 위 장치는 첫 번째 테스트 픽스처이지 사양이 아닙니다.
   코드를 쓰기 전에 같은 클래스의 다른 장치가 어떻게 다른지(개수, 형식, 범위와 이산값, 누락 가능한 선택 필드,
   복합 장치, 여러 동일 장치, 사용 중 변하는 값) 나열하고 각각을 처리합니다.
4. **실패하지 말고 기능을 축소합니다.** 선택 필드가 없으면 생략합니다. 선택 부분을 읽을 수 없으면 레코드에 이유를
   표시합니다. 공급자가 정확한 목록을 만들 수 없을 때만 스캔을 실패시킵니다. 그러면 Sentinel은 마지막 정상 레코드를
   유지하고 오류를 표시합니다.
5. **사실만 보고합니다.** 장치와 커널이 말하는 내용을 보고합니다. Neat 구성 요소의 장치 지원 여부는 공급자가 아니라
   해당 구성 요소의 지원 규칙이 결정합니다.
6. **작업량을 제한합니다.** 공급자는 데몬 내부에서 실행되어 강제 종료할 수 없으므로 절대 블록되지 않아야 하며,
   고장 나거나 악의적인 장치가 무한히 작업하게 해서는 안 됩니다. 각 열거 루프는 1024개
   (`MAX_ENUMERATION_ENTRIES`), 장치별 스캔은 4096개 쿼리(`EnumerationBudget`,
   `src/peripherals/videodev2.rs`의 `MAX_DEVICE_ENUMERATIONS`)로 제한합니다.

## 3. 레코드

공급자는 레코드 목록을 반환합니다.

```json
{"id": "microphone:usb-1-2.3:1.2", "type": "microphone",
 "provider": "daemon.audio.alsa", "details": {"channels": 2, "formats": ["S16_LE"]}}
```

| 필드 | 규칙 |
| --- | --- |
| `id` | 비어 있지 않고 모든 공급자에서 고유하며 안정적이어야 함(위 내용 참조) |
| `type` | 문자로 시작하는 소문자, 숫자, `_`, `-`; 최대 64자; `id`, `type`, `provider`는 사용할 수 없음 |
| `provider` | 공급자 이름. 예: `daemon.audio.alsa` |
| `details` | JSON 객체. 각 필드는 [장치 유형](device-types/README.md)에 문서화된 유형 스키마 |

카탈로그에서 `details`는 유형 이름을 키로 하여 게시됩니다.
`{"id", "type", "provider", "microphone": {...}}`. 클라이언트는 알 수 없는 유형을 JSON으로 읽으므로
API, CLI, Insight 또는 Neat Core의 `details`를 변경하지 않아도 새 유형이 나타납니다.

## 4. 공급자 작성

1. `src/peripherals/<name>/`(또는 `<name>.rs`)을 만들고 `src/peripherals/model.rs`의
   `Provider` 트레이트를 구현합니다.

   ```rust
   impl Provider for MicrophoneProvider {
       fn name(&self) -> &str { "daemon.audio.alsa" }
       // Kernel uevent subsystems that should trigger a rescan.
       fn subsystems(&self) -> &[String] { &self.subsystems } // ["sound"]
       fn discover(&mut self) -> Result<Vec<Record>, ProviderError> { ... }
   }
   ```

2. `src/peripherals/mod.rs`의 `builtin_providers()`에 한 줄을 추가해 등록합니다.
3. 파일 시스템 루트를 주입 가능하게 만들고(예: `with_roots(sys, dev)`), ioctl 호출을 작은 트레이트 뒤에 두어
   하드웨어 없이 테스트할 수 있게 합니다. 카메라 공급자가 이 패턴을 보여 줍니다.
4. 오류를 `ProviderError` 코드 `io.permission_denied`, `io.open`, `peripherals.discovery_failed`로 매핑합니다.

## 5. 테스트

Sentinel과 같은 방식으로 공급자를 한 번 실행해 카탈로그에 추가되는 정확한 내용을 확인합니다.

```bash
simaai-sentinel peripherals --test-provider daemon.audio.alsa
```

이 명령은 데몬과 같은 공급자별 검사로 레코드를 검증하고, 지원 규칙을 적용하고, 결과를 출력하며, 실패 시 0이 아닌
값으로 종료합니다. ID는 공급자 간에도 고유해야 하므로 유형과 공급자별 키를 접두사로 사용합니다.

단위 테스트는 필수입니다.

- 규칙 3에서 나열한 각 변동 축마다 테스트 하나.
- 실제 장치 캡처가 있으면 이를 옮긴 픽스처를 사용하고, 그렇지 않으면 형식에 충실한 합성 픽스처를 사용하며
  각각 무엇인지 표시합니다.
- 권한 거부, 스캔 중 사라지는 장치, 잘못된 응답 등의 오류 경로.

풀 리퀘스트 전에 저장소 CI 단계를 로컬에서 실행합니다.
`cargo fmt --check`, `cargo check --locked`, `cargo test --locked`,
`scripts/build_vulcan_package.sh`.

## 6. 유형 문서화

[템플릿](device-types/TEMPLATE.md)으로 `docs/peripherals/device-types/<type>.md`를 추가하고
[장치 유형 색인](device-types/README.md)에 나열합니다. 애플리케이션 개발자는 이 페이지를 보고 레코드를 읽습니다.
실제 하드웨어에서 확인한 동작과 픽스처에서만 확인한 동작을 구분해 적습니다.

## 체크리스트

- [ ] 커널이 애플리케이션에 필요한 장치 정보를 제공함
- [ ] 읽기 전용이며 설정, 스트리밍, 소유권 획득을 하지 않음
- [ ] 안정적인 속성으로 만들고 유형을 접두사로 한 안정적인 `id`
- [ ] 변동 축을 나열하고 각각 테스트함
- [ ] 선택 데이터가 없을 때 실패하지 않고 기능을 축소함
- [ ] `--test-provider` 출력을 검토함
- [ ] 장치 유형 페이지를 추가하고 색인에 등록함
- [ ] 저장소 CI 단계가 로컬에서 통과함
