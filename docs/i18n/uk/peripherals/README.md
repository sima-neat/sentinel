# Периферійні пристрої

Sentinel веде каталог пристроїв, підключених до Modalix DevKit: які пристрої присутні,
як їх ідентифікувати та що вони можуть робити. Neat Core, Insight, sima-cli2, скрипти
й агенти читають той самий каталог, тому доданий тут тип пристрою стає видимим скрізь одночасно.

Каталог розрахований на розширення. Камери є першим типом пристроїв; додавання іншого типу
(мікрофона, IMU, LiDAR тощо) не потребує змін каталогу, API, CLI або клієнтів.

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

**Провайдер** — це код Rust усередині Sentinel, який виявляє одну родину пристроїв через
інтерфейси ядра й повертає записи. Решту робить Sentinel: пробудження за гарячим підключенням,
усунення брязкоту, ізоляцію помилок, збереження останніх коректних записів провайдера, стабільні
ревізії, журнал змін, API та CLI.

Виявлення навмисно дешеве: коли нічого не змінюється, потік не використовує CPU, працює з nice
на 10 рівнів нижче за демон і об’єднує серії подій та запитів оновлення в одне сканування.

## Сторінки

| Сторінка | Для кого або чого |
| --- | --- |
| [Додавання типу пристрою](adding-a-device-type.md) | Учасники, які додають підтримку нового виду пристроїв |
| [Типи пристроїв](device-types/README.md) | Формат запису кожного підтримуваного типу, починаючи з [камер](device-types/camera.md) |
| [Локальний API агента](../api.md) | `GET /v1/peripherals`, `POST /v1/peripherals/refresh`, опитування через `since_revision` |

## Використання каталогу

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

У застосунку Neat функції `simaai::neat::peripherals::list()` (C++) і
`pyneat.peripherals.list()` (Python) повертають той самий каталог. Кожен пристрій містить подробиці
у JSON (`details_json` / `details`), тому новий тип можна використовувати в застосунках Neat ще до
того, як Core додасть типізовані поля.

<a id="support-rules"></a>

## Правила підтримки

Кожен режим камери містить `supported` і `reason`: чи приймає його `CameraInput` установленого
Neat Core. Це вирішує не Sentinel, а Neat Core. Neat Core встановлює правила у
`/usr/share/simaai-sentinel/support/neat-core.json`, а потік периферії застосовує їх до кожного
режиму перед порівнянням і записом каталогу. Тому оновлення Core збільшує `revision`, як і будь-яка
інша зміна. Sentinel стежить за каталогом правил і повторно застосовує їх без сканування обладнання.
Без Neat Core кожен режим має `supported: false` із причиною, що Core не встановлено.

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

Правила перевіряються в цьому порядку, а перша невдача стає `reason` режиму. Діапазони розмірів
ніколи не позначаються як підтримувані; за наявності `isp_output` режим має бути вихідним розміром ISP.
Поле `support` верхнього рівня каталогу повідомляє `state` (`applied`, `not_installed`, `invalid`
або `stale`, коли після некоректного оновлення діють попередні правила), `source` і `path`.

Sentinel створює `/usr/share/simaai-sentinel/support/`, але не встановлює туди файл, тому Sentinel
і Neat Core не претендують на той самий шлях та можуть незалежно встановлюватися, оновлюватися й
видалятися. Наразі існує лише формат 1; після додавання формату Sentinel продовжить читати старі.
Файл правил новішого, невідомого Sentinel формату залишає чинними попередні правила й повідомляє,
що Sentinel потрібно оновити.

## Параметри демона

| Параметр | Типове значення | Значення |
| --- | --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` | Куди записується каталог і звідки його читає `simaai-sentinel peripherals` |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` | Правила підтримки камер Neat Core |
| `--no-peripherals` | off | Запустити демон без виявлення периферії |
