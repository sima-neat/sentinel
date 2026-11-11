#!/usr/bin/env python3
"""Regenerate golden.json from SoM_Power_Share.py.

Each case in golden.json holds the PMBus transfers of one rail read: the
VOUT_MODE byte and SAMPLES rows of [POUT, VOUT, IOUT] words (null = failed
read). This replays them through the script's own read_rail() and stores what
the script reports. Sentinel's Rust port has to return the same values
(`cargo test`), so run this whenever the hardware team sends a new script.

Usage: python3 hardware/som_power_share/make_golden.py
"""
import hashlib
import importlib.util
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "SoM_Power_Share.py"
GOLDEN = HERE / "golden.json"

spec = importlib.util.spec_from_file_location("som_power_share", SCRIPT)
som = importlib.util.module_from_spec(spec)
spec.loader.exec_module(som)
som.SAMPLE_DELAY = 0            # replay only; nothing to wait for
som.warn = lambda msg: None
WORD_REGS = (som.REG_POUT, som.REG_VOUT, som.REG_IOUT)


def replay(case):
    words = {reg: [row[i] for row in case["samples"]]
             for i, reg in enumerate(WORD_REGS)}
    pages = []

    def i2c_read(addr, reg, mode):
        return case["vout_mode"] if mode == "b" else words[reg].pop(0)

    def set_page(addr, page):
        pages.append(page)
        return case["set_page"]

    som.i2c_read, som.set_page = i2c_read, set_page
    case["expected"] = som.read_rail(case["pmic"], case["rail"])
    case["page"] = pages[-1]


def main():
    golden = json.loads(GOLDEN.read_text())
    for case in golden["cases"]:
        assert len(case["samples"]) == som.SAMPLES, case["name"]
        replay(case)
    golden["script_sha256"] = hashlib.sha256(SCRIPT.read_bytes()).hexdigest()
    lines = ",\n".join("  " + json.dumps(case, separators=(",", ":"))
                       for case in golden["cases"])
    GOLDEN.write_text(
        '{\n "script_sha256": "%s",\n "cases": [\n%s\n ]\n}\n'
        % (golden["script_sha256"], lines))
    print(f"{len(golden['cases'])} cases, script {golden['script_sha256'][:12]}")


if __name__ == "__main__":
    main()
