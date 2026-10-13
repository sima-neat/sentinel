# USB-камери

Камери USB Video Class (UVC), які виявляє через V4L2 провайдер
`camera.v4l2`. Після кожної uevent `video4linux` виконується повторне сканування.

Провайдер обходить `/sys/class/video4linux` і залишає лише вузли, що мають предка
USB, тож вузли платформи та ISP ніколи не відкриваються. Кожен кандидат відкривається з прапорцями
`O_RDONLY | O_NONBLOCK` і отримує лише ioctl-запити (`VIDIOC_QUERYCAP`,
`VIDIOC_ENUM_FMT`, `VIDIOC_ENUM_FRAMESIZES`, `VIDIOC_ENUM_FRAMEINTERVALS`).
Вузли лише з метаданими, лише для виведення та типу memory-to-memory відкидаються після
`VIDIOC_QUERYCAP`, тож UVC-камера з вузлом метаданих з’являється один раз.

## Ідентифікація

`id` — це `camera:v4l2:<16 hex digits>`, хеш FNV-1a від `identity.stable_key`
(`sysfs:<USB topology>:interface=<bInterfaceNumber>:index=<node index>`). Він
не змінюється після повторного підключення до того самого порту та перенумерації `/dev/videoN`. Інший
порт означає іншу камеру, а однакові камери в різних портах
мають різні id.

## Поля

| Поле | Наявність | Значення |
| --- | --- | --- |
| `type` | завжди | `camera` |
| `id` | завжди | Див. вище |
| `backend` | завжди | `v4l2` (USB-камера) |
| `model` | якщо відомо | Рядок USB `product`, інакше назва картки (card) драйвера |
| `device_path` | завжди | `/dev/videoN`; лише для маршрутизації, не для ідентифікації |
| `by_id_path` | за наявності посилання udev | Посилання `/dev/v4l/by-id/...`, яке вказує на `device_path` |
| `identity` | завжди | `stable_key`, `topology`, `interface`, `node_index` і, якщо USB-пристрій їх повідомляє, `vendor_id`, `product_id`, `serial`, `manufacturer` та `speed` (швидкість із sysfs у Мбіт/с у тому вигляді, як її виводить ядро, наприклад `"480"`) |
| `availability` | завжди | `{"state": "unknown", "reason": ...}`: виявлення ніколи не відкриває потік, тому не може визначити, чи використовується камера |
| `modes` | завжди | Що камера може виводити; див. нижче |

## Режими

Один режим на кожну комбінацію формату й розміру кадру, відсортовані за форматом, а потім за розміром.
Одноплощинні та багатоплощинні переліки одного формату об’єднуються.

| Поле | Наявність | Значення |
| --- | --- | --- |
| `format` | завжди | V4L2 FourCC, наприклад `MJPG`, `YUYV`, `NV12` |
| `format_description` | якщо драйвер його надає | Опис із `VIDIOC_ENUM_FMT`, наприклад `Motion-JPEG` |
| `width`, `height` | дискретні розміри | Розмір кадру |
| `size_range` | ступінчасті або неперервні розміри | `type` (`stepwise` або `continuous`), `min_width`, `min_height`, `max_width`, `max_height`, `step_width`, `step_height` |
| `frame_intervals` | завжди | Для кожного опитаного розміру (`width`, `height`) — кожен інтервал, який оголошує пристрій: `{"type": "discrete", "numerator", "denominator"}` або `{"type": "stepwise" or "continuous", "minimum", "maximum", "step"}` |

Діапазон розмірів опитується за його мінімальним і максимальним розміром. Розмір без інтервалів
кадрів не має режиму. Кожен режим також має `available: false` і `reason`:
USB-камери не мають оверлея камери на платі (див. [каталог](../api.md)).

## Обмеження та помилки

Будь-який збій спричиняє невдачу сканування провайдера, про що повідомляється в полі
`errors` каталогу:

- `io.permission_denied`: `EACCES` від вузла або sysfs, або `EPERM` під час
  відкриття вузла.
- `io.open`: будь-яка інша помилка драйвера або sysfs; список, довший за 1024 елементи;
  понад 4096 запитів переліку для одного пристрою; некоректний розмір або
  інтервал.
- `peripherals.discovery_failed`: USB-камера без номера інтерфейсу або
  індексу вузла.

Камеру, від’єднану під час сканування, буде пропущено, а не зараховано як збій сканування, — щонайпізніше
під час повторного сканування, яке запускає її від’єднання.

## Приклад

```json
{"type": "camera", "id": "camera:v4l2:295faa7ac0d61654", "backend": "v4l2",
 "model": "HD Pro Webcam C920", "device_path": "/dev/video97",
 "by_id_path": "/dev/v4l/by-id/usb-046d_HD_Pro_Webcam_C920_A1B2-video-index0",
 "identity": {"stable_key": "sysfs:devices/pci0000:00/usb1/1-2.3:interface=00:index=0",
              "topology": "devices/pci0000:00/usb1/1-2.3", "interface": "00",
              "node_index": "0", "vendor_id": "046d", "product_id": "082d",
              "serial": "A1B2", "manufacturer": "Logitech", "speed": "480"},
 "availability": {"state": "unknown", "reason": "V4L2 does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "MJPG", "format_description": "Motion-JPEG",
            "width": 1920, "height": 1080,
            "frame_intervals": [{"width": 1920, "height": 1080, "intervals": [
              {"type": "discrete", "numerator": 1, "denominator": 30}]}]}]}
```
