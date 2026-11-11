#!/usr/bin/env python3
"""
SoM power telemetry - combined PMBus readout for both SoM PMICs.

  PMIC 1 = 0x4D  (rails 0.68, 0.9, 1.8, 3.3)
  PMIC 2 = 0x4F  (rails 0.75, 1.2, 0.5, 1.05)

Total SoM power = sum of Pout of all rails on both PMICs.

Run with -h for usage examples.
"""
import argparse
import os
import subprocess
import sys
import time

# --- CONFIGURATION ----------------------------------------------------------
I2C_BUS = '3'
I2CGET = '/usr/sbin/i2cget'
I2CSET = '/usr/sbin/i2cset'

SAMPLES = 50              # reads averaged per measurement
SAMPLE_DELAY = 0.002      # seconds between reads
DEFAULT_VOUT_EXP = -8     # used if VOUT_MODE cannot be read

# Each PMIC: I2C address, rail name -> PMBus page, and the "core" rail that
# needs the 8-bit mask + fixed scaling fix for IOUT/POUT.
PMICS = {
    "1": {
        "addr": "0x4d",
        "core_rail": "0.68",
        "rails": {"0.68": 0x00, "0.9": 0x01, "1.8": 0x02, "3.3": 0x03},
    },
    "2": {
        "addr": "0x4f",
        "core_rail": "0.75",
        "rails": {"0.75": 0x00, "1.2": 0x01, "0.5": 0x02, "1.05": 0x03},
    },
}

# PMBus Commands
REG_PAGE = 0x00
REG_VOUT_MODE = 0x20
REG_VOUT = 0x8B
REG_IOUT = 0x8C
REG_POUT = 0x96


def pmic_label(key):
    return f"PMIC {key} ({PMICS[key]['addr']})"


# --- LOW-LEVEL I2C ----------------------------------------------------------
def warn(msg):
    print(f"  [WARN] {msg}", file=sys.stderr)


def i2c_read(addr, reg, mode):
    """Read a byte ('b') or word ('w'). Returns int, or None on failure."""
    cmd = [I2CGET, '-y', I2C_BUS, addr, hex(reg), mode]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=1)
    except (OSError, subprocess.TimeoutExpired):
        return None
    out = res.stdout.strip()
    if res.returncode != 0 or not out or "Error" in out:
        return None
    try:
        return int(out, 16)
    except ValueError:
        return None


def set_page(addr, page):
    cmd = [I2CSET, '-y', I2C_BUS, addr, hex(REG_PAGE), hex(page)]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=1)
    except (OSError, subprocess.TimeoutExpired):
        return False
    return res.returncode == 0


# --- DECODING ---------------------------------------------------------------
def decode_linear11(val):
    exp = (val >> 11) & 0x1F
    if exp > 15:
        exp -= 32
    mant = val & 0x7FF
    if mant > 1023:
        mant -= 2048
    return mant * (2 ** exp)


def decode(raw, reg, is_core):
    if is_core:
        # Specific fix for the high-current core rail
        if reg == REG_IOUT:
            return (raw & 0xFF) * 2 ** -3
        if reg == REG_POUT:
            return (raw & 0xFF) * 2 ** -2
    return decode_linear11(raw)


def read_vout_exp(addr):
    raw = i2c_read(addr, REG_VOUT_MODE, 'b')
    if raw is None:
        return DEFAULT_VOUT_EXP
    e = raw & 0x1F
    return e - 32 if e > 15 else e


def sample_many(addr, regs):
    """Average SAMPLES reads of several registers, round-robin.

    regs: {name: (reg, is_core, vout_exp)}  (vout_exp only for VOUT)
    Each loop reads every register once, so all values are averaged over the
    same time window. Returns {name: average or None if all reads failed}.
    """
    totals = dict.fromkeys(regs, 0.0)
    counts = dict.fromkeys(regs, 0)
    for _ in range(SAMPLES):
        for name, (reg, is_core, vout_exp) in regs.items():
            raw = i2c_read(addr, reg, 'w')
            if raw is None:
                continue
            if vout_exp is not None:
                totals[name] += raw * (2 ** vout_exp)   # VOUT uses LINEAR16
            else:
                totals[name] += decode(raw, reg, is_core)
            counts[name] += 1
        time.sleep(SAMPLE_DELAY)
    return {n: totals[n] / counts[n] if counts[n] else None for n in regs}


# --- MEASUREMENTS -----------------------------------------------------------
def read_rail(key, rail):
    pmic = PMICS[key]
    addr = pmic["addr"]
    if not set_page(addr, pmic["rails"][rail]):
        warn(f"{pmic_label(key)}: failed to select page for rail {rail}V")
        return dict.fromkeys(["vout", "iout", "pout"])
    is_core = rail == pmic["core_rail"]
    v_exp = read_vout_exp(addr)
    d = sample_many(addr, {
        "pout": (REG_POUT, is_core, None),
        "vout": (REG_VOUT, False, v_exp),
        "iout": (REG_IOUT, is_core, None),
    })

    # Core rail: READ_POUT has 0.25 W steps, while Vout x Iout resolves
    # ~0.085 W (0.125 A steps), so use the product for Pout.
    if is_core and d["vout"] is not None and d["iout"] is not None:
        d["pout"] = d["vout"] * d["iout"]
    return d


# --- OUTPUT -----------------------------------------------------------------
def fmt(v, width, prec):
    return f"{v:{width}.{prec}f}" if v is not None else f"{'N/A':>{width}}"


def safe_sum(values):
    """Sum ignoring None; also report whether any value was missing."""
    vals = list(values)
    return sum(v for v in vals if v is not None), any(v is None for v in vals)


def print_rail(key, rail, d):
    print(f"\n[RAIL: {rail}V - {pmic_label(key)}]")
    print(f"  Vout : {fmt(d['vout'], 7, 3)} V")
    print(f"  Iout : {fmt(d['iout'], 7, 2)} A")
    print(f"  Pout : {fmt(d['pout'], 7, 2)} W")


# Summary table layout: label column + fixed-width numeric columns
NAME_W = 18
COL_W = 11


def report_full(pmic_keys, verbose):
    results = {}
    for key in pmic_keys:
        rails = {r: read_rail(key, r) for r in PMICS[key]["rails"]}
        results[key] = rails
        if verbose:
            print(f"\n--- Telemetry for {pmic_label(key)}, Bus {I2C_BUS} ---")
            for r, d in rails.items():
                print_rail(key, r, d)

    headers = ["Pout (W)"]
    width = NAME_W + COL_W * len(headers)

    def row(label, cells):
        print(("  " + f"{label:<{NAME_W}}" + "".join(cells)).rstrip())

    print("\n  " + " SoM POWER SUMMARY ".center(width, "="))
    row("PMIC", [f"{h:>{COL_W}}" for h in headers])
    print("  " + "-" * width)

    tot_pout, missing = 0.0, False
    for key, rails in results.items():
        pout, pout_missing = safe_sum(d["pout"] for d in rails.values())
        missing |= pout_missing
        tot_pout += pout
        row(pmic_label(key), [fmt(pout, COL_W, 2)])

    print("  " + "-" * width)
    label = "Total SoM Power" if len(pmic_keys) == len(PMICS) else \
        "Total (" + ", ".join(f"PMIC {k}" for k in pmic_keys) + ")"
    row(label, [fmt(tot_pout, COL_W, 2)])
    print("  " + "=" * width)
    if missing:
        print("  [!] Some readings failed (N/A) and were excluded from totals.")


def report_rails(targets):
    pouts = []
    for key, rail in targets:
        d = read_rail(key, rail)
        print_rail(key, rail, d)
        pouts.append(d["pout"])
    if len(targets) > 1:
        total, missing = safe_sum(pouts)
        names = ", ".join(f"{r}V" for _, r in targets)
        print(f"\n  Total Pout ({names}) : {total:.2f} W")
        if missing:
            print("  [!] Some readings failed (N/A) and were excluded from the total.")


# --- RAIL SELECTION ---------------------------------------------------------
def norm_pmic(s):
    """Accept '1', '2', 'pmic1', 'PMIC2' -> '1' / '2'."""
    s = s.strip().lower()
    return s[4:] if s.startswith("pmic") else s


def same_rail(user, name):
    user = user.strip().lower().rstrip("v")
    try:
        return float(user) == float(name)
    except ValueError:
        return user == name


def resolve_rails(specs, pmic_keys):
    """Turn ['0.68', '2:1.2', ...] into [(pmic_key, rail_name), ...]."""
    targets = []
    for spec in specs:
        if ":" in spec:
            pk, rn = spec.split(":", 1)
            pk = norm_pmic(pk)
            if pk not in PMICS:
                raise ValueError(f"unknown PMIC '{pk}' in '{spec}' "
                                 f"(choose from {', '.join(PMICS)})")
            keys = [pk]
        else:
            rn, keys = spec, pmic_keys
        matches = [(k, n) for k in keys for n in PMICS[k]["rails"] if same_rail(rn, n)]
        if not matches:
            raise ValueError(f"no rail '{spec}' found (use -l to list rails)")
        if len(matches) > 1:
            opts = ", ".join(f"{k}:{n}" for k, n in matches)
            raise ValueError(f"rail '{spec}' is ambiguous, use one of: {opts}")
        if matches[0] not in targets:
            targets.append(matches[0])
    return targets


def list_rails():
    for key, p in PMICS.items():
        print(f"{pmic_label(key)}, bus {I2C_BUS}:")
        for name in p["rails"]:
            print(f"  {name:>5} V")


# --- MAIN -------------------------------------------------------------------
# (arguments, description) - shown under -h using the script's actual name
EXAMPLES = [
    ("",               "summary: per-PMIC Pout + SoM total"),
    ("-v",             "verbose: every rail of each PMIC + summary"),
    ("-r 0.68",        "just the 0.68 V rail"),
    ("-r 0.68 3.3 1.2", "several rails + their summed Pout"),
    ("-r 2:0.75",      "rail on an explicit PMIC (1 or 2)"),
    ("-p 1 -v",        "PMIC 1 only, all its rails"),
    ("-l",             "list available rails"),
]


def examples_text(prog):
    cmds = [f"python3 {prog} {args}".rstrip() for args, _ in EXAMPLES]
    width = max(len(c) for c in cmds)
    lines = [f"  {c:<{width}}  # {desc}" for c, (_, desc) in zip(cmds, EXAMPLES)]
    return "Examples:\n" + "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(
        description="SoM power telemetry from both PMICs "
                    "(PMIC 1 = 0x4D, PMIC 2 = 0x4F).",
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.epilog = examples_text(parser.prog)
    parser.add_argument("-r", "--rail", nargs="+", metavar="RAIL",
                        help="only read these rail(s), e.g. 0.68 3.3 or 2:0.75 "
                             "(space or comma separated)")
    parser.add_argument("-p", "--pmic", type=norm_pmic, choices=list(PMICS),
                        help="limit to one PMIC (1 or 2)")
    parser.add_argument("-v", "-V", "--verbose", action="store_true",
                        help="show every rail of each PMIC, not just the summary")
    parser.add_argument("-l", "--list", action="store_true",
                        help="list available rails and exit")
    args = parser.parse_args()

    if args.list:
        list_rails()
        return 0
    for tool in (I2CGET, I2CSET):
        if not os.path.exists(tool):
            parser.error(f"{tool} not found (install i2c-tools)")

    pmic_keys = [args.pmic] if args.pmic else list(PMICS)
    if args.rail:
        specs = [s for item in args.rail for s in item.split(",") if s.strip()]
        try:
            targets = resolve_rails(specs, pmic_keys)
        except ValueError as e:
            parser.error(str(e))
        report_rails(targets)
    else:
        report_full(pmic_keys, args.verbose)
    return 0


if __name__ == "__main__":
    sys.exit(main())
