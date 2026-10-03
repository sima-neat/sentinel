# <Type name>

<!-- One paragraph: what this type covers and which devices are in its class. -->

- **Type token:** `<type>`
- **Providers:** `<provider name>` (built-in or external)
- **Rescan triggers:** `<uevent subsystems>`

## Identity

How `id` is formed, from which stable attributes, and what keeps it stable
across replugs, reboots and renumbering. Example: `<type>:<stable key>`.

## Details

| Field | Type | Always present | Meaning | Source |
| --- | --- | --- | --- | --- |
| `<field>` | string | yes | <what it means> | <sysfs file, ioctl, vendor API> |

## Example record

```json
{"id": "<type>:...", "type": "<type>", "provider": "...", "<type>": {}}
```

## Variation covered

List the ways devices of this class differ and how each is handled
(counts, formats, ranges, optional fields, composite devices, several
identical devices, values that change while in use).

## Support rules

Whether a Neat component's rules classify this type, and which fields they
read. Write "none" if the type has no support classification.

## Verification

| Behaviour | Real hardware (which device) | Fixtures only |
| --- | --- | --- |
| <behaviour> | <device> | |
