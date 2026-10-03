#!/usr/bin/env python3
"""Reference external peripheral provider for SiMa Sentinel.

Prints one provider protocol v1 document on stdout. Replace discover() with
real, read-only discovery for your device type; keep the output shape.
"""

import json
import sys

PROVIDER = "example.sensor"  # must equal "name" in the manifest
TYPE = "sensor"  # lowercase; becomes the catalog key for details


def discover():
    # Read-only discovery goes here: sysfs, /proc, a vendor query API, ...
    # Never configure, stream from, or take ownership of the device.
    return [
        {
            "id": "sensor:example-0",  # stable across replugs and reboots
            "type": TYPE,
            "provider": PROVIDER,
            "details": {"model": "Example Sensor", "channels": 2},
        }
    ]


def main():
    try:
        document = {"schema_version": 1, "ok": True, "records": discover()}
    except PermissionError as error:
        document = {"schema_version": 1, "ok": False,
                    "error": {"code": "io.permission_denied", "reason": str(error)}}
    except Exception as error:  # report, never crash silently
        document = {"schema_version": 1, "ok": False,
                    "error": {"code": "peripherals.discovery_failed", "reason": str(error)}}
    json.dump(document, sys.stdout)
    return 0 if document["ok"] else 2


if __name__ == "__main__":
    sys.exit(main())
