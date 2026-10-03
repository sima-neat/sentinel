# <Type name>

What this type covers and which devices are in its class.

- **Type token:** `<type>`
- **Provider:** `<provider name>`, rescans on `<uevent subsystems>`
- **Identity:** how `id` is formed and why it is stable

| Field | Type | Always present | Meaning | Source |
| --- | --- | --- | --- | --- |
| `<field>` | string | yes | | sysfs file, ioctl |

**Variation covered:** how devices of this class differ and how each is handled.

**Verification:** which behaviour was checked on real hardware (which device)
and which only on fixtures.
