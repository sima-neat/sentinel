# <Type name>

<!-- One paragraph: what this type covers and which devices are in its class. -->

- **유형 토큰:** `<type>`
- **공급자:** `<provider name>`
- **재검색 트리거:** `<uevent subsystems>`

## 식별자

`id`를 구성하는 안정적인 속성과 재연결, 재부팅, 번호 변경 후에도 안정성을 유지하는 요소를 설명합니다.
예: `<type>:<stable key>`.

## 세부 정보

| 필드 | 유형 | 항상 존재 | 의미 | 출처 |
| --- | --- | --- | --- | --- |
| `<field>` | string | yes | <what it means> | <sysfs file, ioctl, vendor API> |

## 레코드 예시

```json
{"id": "<type>:...", "type": "<type>", "provider": "...", "<type>": {}}
```

## 지원하는 변동

이 클래스의 장치가 달라지는 방식과 각각의 처리 방법을 나열합니다
(개수, 형식, 범위, 선택 필드, 복합 장치, 여러 동일 장치, 사용 중 변하는 값).

## 지원 규칙

Neat 구성 요소의 규칙이 이 유형을 분류하는지, 어떤 필드를 읽는지 설명합니다.
지원 분류가 없으면 "없음"이라고 적습니다.

## 검증

| 동작 | 실제 하드웨어(장치) | 픽스처만 사용 |
| --- | --- | --- |
| <behaviour> | <device> | |
