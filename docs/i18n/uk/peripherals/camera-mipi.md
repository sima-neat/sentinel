# Камери MIPI CSI-2

Провайдер `camera.mipi` повідомляє про сенсори зображення за ISP Modalix.
Він виконує повторне сканування за uevent `media` і `video4linux`. Виявлення відкриває вузли пристроїв
лише для читання й надсилає тільки ioctl-запити; воно ніколи не встановлює формат чи зв’язок і
ніколи не запускає потік.

Кожен `/dev/mediaN` драйвера `simaai-v4l2-vid` читається за допомогою
`MEDIA_IOC_G_TOPOLOGY`, і кожна сутність `MEDIA_ENT_F_CAM_SENSOR` у його графі є
однією камерою. Медіапристрої інших драйверів (наприклад, `uvcvideo`) ігноруються.

## Ідентифікація

`id` — це `camera:<sensor entity name>`, наприклад `camera:imx477 5-001a`: драйвер
сенсора, шина I2C та адреса. libcamera використовує те саме ім’я, тож це
також ім’я, яке приймає `CameraInput`. Два сенсори з однаковим ім’ям, наприклад
на двох медіапристроях, мали б спільний id, тому натомість сканування завершується
помилкою, у якій названо обидва медіапристрої.

## Поля

| Поле | Наявність | Значення |
| --- | --- | --- |
| `type`, `id` | завжди | `camera` та ідентифікатор, описаний вище |
| `backend` | завжди | `mipi` |
| `model` | якщо ім’я його містить | Перше слово імені сутності, наприклад `imx477` |
| `availability` | завжди | `{"state": "unknown", "reason": ...}`: медіаконтролер не має стану володіння, доступного лише для читання |
| `camera_name` | завжди | Ім’я сутності сенсора для передавання в `CameraInput` |
| `media_device` | завжди | `/dev/mediaN` (лише для маршрутизації; не для ідентифікації) |
| `bus_info` | якщо повідомляється | Шина медіапристрою з `MEDIA_IOC_DEVICE_INFO`, наприклад `platform:csi2video@1` |
| `isp` | завжди | `{"state": "available", "device_path", "device_paths"}` або `{"state": "unavailable", "reason"}` разом із `modes: []` |
| `csi_receiver` | якщо є зв’язок | Сутність, з якою зв’язаний вихідний пад (source pad) сенсора, наприклад `csidev-40c3000.csi` |
| `sensor_timing` | якщо вдається прочитати | `pixel_rate` (пікселів/с), `hblank_min`, `vblank_min`, `width`, `height`, прочитані з `/dev/v4l-subdevN` сенсора |
| `max_fps` | за наявності `sensor_timing` | `pixel_rate / ((width + hblank_min) * (height + vblank_min))`, з точністю до двох знаків після коми |

`sensor_timing` береться з інтерфейсу субпристрою сенсора (названого через
`/sys/dev/char/<major>:<minor>`): активний формат зв’язаного вихідного пада
(`VIDIOC_SUBDEV_G_FMT`), поточне значення `V4L2_CID_PIXEL_RATE` та мінімуми
`V4L2_CID_HBLANK` і `V4L2_CID_VBLANK`. Поле пропускається, якщо будь-якого з них
бракує.

Вихідні вузли ISP — це записи `video4linux` з іменем
`isp_v4l2-vid-cap-out` і card `arm-isp-out`. Якщо їх кілька, повідомляються лише
режими, спільні для всіх.

## Режими

Режими — це вихідні формати та дискретні розміри ISP, тобто те, що може захоплювати
`CameraInput`. Кожен режим має `format` (FourCC), `width`, `height`
і `isp_output: true`. Режим містить `frame_intervals` у тій самій формі, що й
для USB-камер, лише якщо ISP повідомляє інтервали для свого розміру.
ISP DevKit їх не повідомляє, тому його режими не мають частоти кадрів; `max_fps` і
`sensor_timing` описують обмеження сенсора.

## Помилки

| Код | Коли |
| --- | --- |
| `io.permission_denied` | Медіапристрій неможливо відкрити (`EACCES`, `EPERM`) або неможливо отримати перелік вмісту `/dev` (`EACCES`) |
| `io.open` | Будь-який інший збій відкриття медіапристрою або запиту до нього |
| `peripherals.discovery_failed` | Сутність сенсора без імені або два сенсори з однаковим ім’ям (їхні id збіглися б) |

Пристрій, що зникає посеред сканування, пропускається. Збої ISP ніколи не спричиняють невдачу сканування;
вони роблять `isp` недоступним із зазначенням причини.

## Приклад

IMX477 на Modalix DevKit зі скороченим переліком режимів (загалом дев’ять: три
формати, три розміри). Граф і розміри ISP переписано з
DevKit; значення таймінгів сенсора наведено так, як їх повідомляє DevKit, але
вони покриті лише тестами.

```json
{"type": "camera", "id": "camera:imx477 5-001a", "model": "imx477",
 "availability": {"state": "unknown", "reason": "The media controller does not expose a reliable read-only ownership state; discovery does not acquire, configure, or stream from the camera."},
 "modes": [{"format": "AR24", "width": 1920, "height": 1080, "isp_output": true},
           {"format": "AR24", "width": 2048, "height": 1080, "isp_output": true}],
 "backend": "mipi", "camera_name": "imx477 5-001a", "media_device": "/dev/media0",
 "bus_info": "platform:csi2video@1",
 "isp": {"state": "available", "device_path": "/dev/video1", "device_paths": ["/dev/video1"]},
 "csi_receiver": "csidev-40c3000.csi",
 "sensor_timing": {"pixel_rate": 840000000, "hblank_min": 9332, "vblank_min": 48,
                   "width": 1920, "height": 1080},
 "max_fps": 66.18}
```
