# Периферійні пристрої

Sentinel зберігає каталог пристроїв, підключених до Modalix DevKit: які пристрої присутні, як їх ідентифікувати та що вони можуть робити. Neat Core, Insight, sima-cli2, скрипти та агенти всі читають той самий каталог, тому тип пристрою, доданий тут, одразу стає видимим всюди.

Каталог створений для зростання. Камери та мікрофони є вбудованими типами пристроїв; додавання іншого типу (IMU, LiDAR тощо) не потребує змін у каталозі, API, CLI або клієнтах.

## Як це працює

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

**Постачальник** – це код на Rust всередині Sentinel, який виявляє одну родину пристроїв з інтерфейсів ядра та повертає записи. Sentinel робить усе інше: реагує на гаряче підключення, усуває контактні помилкові спрацьовування, ізолює збої, зберігає останні правильні записи збійного постачальника, стабільні ревізії, журнал змін, API та CLI.

Виявлення за замовчуванням дешеве: потік не використовує процесор, поки нічого не змінюється, працює на 10 рівнів нижче демона, та об’єднує серії подій і запити на оновлення в одне сканування.

## Сторінки

| Сторінка | Для |
| --- | --- |
| [Додавання типу пристрою](adding-a-device-type.md) | Учасники, що додають підтримку нового типу пристрою |
| [Типи пристроїв](device-types/README.md) | Формат запису для кожного підтримуваного типу: [камери](device-types/camera.md), [мікрофони](device-types/microphone.md) |
| [Місцевий агент API](../api.md) | `GET /v1/peripherals`, `POST /v1/peripherals/refresh`, опитування з `since_revision` |

## Використання каталогу

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

З програми Neat, `simaai::neat::peripherals::list()` (C++) та `pyneat.peripherals.list()` (Python) повертають той самий каталог. Кожен пристрій зберігає свої деталі як JSON (`details_json` / `details`), тому новий тип пристрою можна використовувати у програмах Neat до того, як Core додасть для нього типізовані поля.

<a id="support-rules"></a>

## Правила підтримки

Кожен режим камери має `supported` і `reason`: чи приймає його встановлений Neat Core `CameraInput`. Sentinel цього не визначає. Neat Core встановлює свої правила у `/usr/share/simaai-sentinel/support/neat-core.json`, а потік периферійних пристроїв застосовує їх до кожного режиму перед порівнянням і записом каталогу, тому оновлення Core збільшує `revision` так само, як будь-яка інша зміна. Sentinel стежить за директорією і повторно застосовує правила без повторного сканування апаратури. Без Neat Core кожен режим є `supported: false` з причиною, що Core не встановлено.

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

Правила перевіряються в такому порядку, і перший збій стає `reason` режиму. Діапазони розмірів ніколи не позначаються як підтримувані; `isp_output`, якщо присутній, вимагає, щоб режим був розміром виходу ISP. Вищий рівень каталогу `support` повідомляє `state` (`applied`, `not_installed`, `invalid` або `stale` у випадку, якщо недійсне оновлення залишило в використанні попередні правила), `source` та `path`.

Sentinel створює `/usr/share/simaai-sentinel/support/`, але ніколи не встановлює в ньому файл, тому Sentinel та Neat Core ніколи не претендують на той самий шлях і встановлюють, оновлюють або видаляють незалежно. Формат 1 — це єдиний формат на даний момент; коли додається новий формат, Sentinel продовжує читати старі. Файл правил у форматі, новішому за Sentinel knows, залишає попередні правила в користуванні та повідомляє, що Sentinel потребує оновлення.

## Параметри демона

| Опція | За замовчуванням | Значення |
| --- | --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` | Де каталог записується та читається `simaai-sentinel peripherals` |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` | Правила підтримки камер Neat Core |
| `--no-peripherals` | вимкнено | Запустіть демон без виявлення периферійних пристроїв |
