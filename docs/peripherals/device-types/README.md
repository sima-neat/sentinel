# Device types

Each device type has a page describing its record: how devices are
identified, every field of its details, and what was verified on real
hardware. Application developers read these pages to use the catalog.

| Type | Providers | Page |
| --- | --- | --- |
| `camera` | `daemon.camera.mipi`, `daemon.camera.v4l2` | [Cameras](camera.md) |

## Adding a type

1. Copy [TEMPLATE.md](TEMPLATE.md) to `<type>.md` and fill in every section.
2. Add a row to the table above.
3. Follow [adding a device type](../adding-a-device-type.md) for the provider
   itself.

Fields may be added to a type at any time; clients ignore fields they do not
know. Renaming or removing a field, or changing its meaning, breaks clients
and needs a new field name instead.
