//! Rust port of the hardware team's `SoM_Power_Share.py`.
//!
//! The measurement method is the hardware team's: which registers are read,
//! how they are decoded, the rail map and every constant below come from their
//! script, kept unmodified at `hardware/som_power_share/SoM_Power_Share.py`.
//! Names follow the script so the two read side by side. Sentinel only swaps
//! the script's `i2cget`/`i2cset` subprocesses for in-process SMBus transfers.
//!
//! Change this file only to follow a new script from the hardware team, then
//! regenerate `hardware/som_power_share/golden.json` with `make_golden.py`.

use std::thread;
use std::time::Duration;

use super::PowerI2cBackend;

/// SHA-256 of the script this port follows.
pub const SCRIPT_SHA256: &str = "be3d3609012c56a2022f0f491e795a1a3eed50260d3faf304208013be0659914";
pub const METHOD: &str = "som_power_share";

// --- CONFIGURATION ---------------------------------------------------------
pub const I2C_BUS: i32 = 3;

pub const SAMPLES: u32 = 50; // reads averaged per measurement
pub const SAMPLE_DELAY: Duration = Duration::from_millis(2); // between reads
const DEFAULT_VOUT_EXP: i32 = -8; // used if VOUT_MODE cannot be read

/// Each PMIC: I2C address, rail name -> PMBus page, and the "core" rail that
/// needs the 8-bit mask + fixed scaling fix for IOUT/POUT.
pub struct Pmic {
    pub key: &'static str,
    pub addr: u8,
    pub core_rail: &'static str,
    pub rails: [(&'static str, u8); 4],
}

pub const PMICS: [Pmic; 2] = [
    Pmic {
        key: "1",
        addr: 0x4d,
        core_rail: "0.68",
        rails: [("0.68", 0x00), ("0.9", 0x01), ("1.8", 0x02), ("3.3", 0x03)],
    },
    Pmic {
        key: "2",
        addr: 0x4f,
        core_rail: "0.75",
        rails: [("0.75", 0x00), ("1.2", 0x01), ("0.5", 0x02), ("1.05", 0x03)],
    },
];

// PMBus Commands
const REG_VOUT_MODE: u8 = 0x20;
const REG_VOUT: u8 = 0x8B;
const REG_IOUT: u8 = 0x8C;
const REG_POUT: u8 = 0x96;

/// One rail as the script reports it. `None` means every read failed;
/// `error` keeps the last transfer error so Sentinel can say why.
#[derive(Debug, Default, PartialEq)]
pub struct RailReading {
    pub vout: Option<f64>,
    pub iout: Option<f64>,
    pub pout: Option<f64>,
    pub error: Option<String>,
}

/// The script rail served by this PMIC address and page.
pub fn find_rail(addr: u8, page: u8) -> Option<(&'static Pmic, &'static str)> {
    let pmic = PMICS.iter().find(|pmic| pmic.addr == addr)?;
    let (rail, _) = pmic.rails.iter().find(|(_, p)| *p == page)?;
    Some((pmic, rail))
}

// --- LOW-LEVEL I2C ---------------------------------------------------------
struct I2c<'a> {
    backend: &'a dyn PowerI2cBackend,
    addr: u8,
    error: Option<String>,
}

impl I2c<'_> {
    fn keep<T>(&mut self, result: Result<T, String>) -> Option<T> {
        result.map_err(|error| self.error = Some(error)).ok()
    }

    fn read_byte(&mut self, reg: u8) -> Option<u8> {
        self.keep(self.backend.read_register(I2C_BUS, self.addr, reg))
    }

    fn read_word(&mut self, reg: u8) -> Option<u16> {
        self.keep(self.backend.read_word(I2C_BUS, self.addr, reg))
    }

    fn set_page(&mut self, page: u8) -> bool {
        self.keep(self.backend.set_page(I2C_BUS, self.addr, page))
            .is_some()
    }
}

// --- DECODING --------------------------------------------------------------
fn decode_linear11(val: u16) -> f64 {
    let mut exp = i32::from(val >> 11) & 0x1F;
    if exp > 15 {
        exp -= 32;
    }
    let mut mant = i32::from(val & 0x7FF);
    if mant > 1023 {
        mant -= 2048;
    }
    f64::from(mant) * 2f64.powi(exp)
}

fn decode(raw: u16, reg: u8, is_core: bool) -> f64 {
    if is_core {
        // Specific fix for the high-current core rail
        if reg == REG_IOUT {
            return f64::from(raw & 0xFF) * 2f64.powi(-3);
        }
        if reg == REG_POUT {
            return f64::from(raw & 0xFF) * 2f64.powi(-2);
        }
    }
    decode_linear11(raw)
}

fn read_vout_exp(i2c: &mut I2c) -> i32 {
    let Some(raw) = i2c.read_byte(REG_VOUT_MODE) else {
        return DEFAULT_VOUT_EXP;
    };
    let e = i32::from(raw & 0x1F);
    if e > 15 {
        e - 32
    } else {
        e
    }
}

/// Average `SAMPLES` reads of several registers, round-robin.
///
/// regs: (register, is_core, vout_exp), with vout_exp set only for VOUT.
/// Each loop reads every register once, so all values are averaged over the
/// same time window. An entry is `None` if all its reads failed.
fn sample_many(i2c: &mut I2c, regs: [(u8, bool, Option<i32>); 3]) -> [Option<f64>; 3] {
    let mut totals = [0.0; 3];
    let mut counts = [0u32; 3];
    for _ in 0..SAMPLES {
        for (slot, (reg, is_core, vout_exp)) in regs.into_iter().enumerate() {
            let Some(raw) = i2c.read_word(reg) else {
                continue;
            };
            totals[slot] += match vout_exp {
                Some(exp) => f64::from(raw) * 2f64.powi(exp), // VOUT uses LINEAR16
                None => decode(raw, reg, is_core),
            };
            counts[slot] += 1;
        }
        thread::sleep(SAMPLE_DELAY);
    }
    std::array::from_fn(|slot| (counts[slot] > 0).then(|| totals[slot] / f64::from(counts[slot])))
}

// --- MEASUREMENTS ----------------------------------------------------------
pub fn read_rail(backend: &dyn PowerI2cBackend, pmic: &Pmic, rail: &str) -> RailReading {
    let mut i2c = I2c {
        backend,
        addr: pmic.addr,
        error: None,
    };
    let Some(&(_, page)) = pmic.rails.iter().find(|(name, _)| *name == rail) else {
        return RailReading {
            error: Some(format!("PMIC {} has no rail {rail}V", pmic.key)),
            ..RailReading::default()
        };
    };
    if !i2c.set_page(page) {
        return RailReading {
            error: i2c.error,
            ..RailReading::default()
        };
    }
    let is_core = rail == pmic.core_rail;
    let v_exp = read_vout_exp(&mut i2c);
    let [mut pout, vout, iout] = sample_many(
        &mut i2c,
        [
            (REG_POUT, is_core, None),
            (REG_VOUT, false, Some(v_exp)),
            (REG_IOUT, is_core, None),
        ],
    );

    // Core rail: READ_POUT has 0.25 W steps, while Vout x Iout resolves
    // ~0.085 W (0.125 A steps), so use the product for Pout.
    if let (true, Some(vout), Some(iout)) = (is_core, vout, iout) {
        pout = Some(vout * iout);
    }
    RailReading {
        vout,
        iout,
        pout,
        error: i2c.error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const SCRIPT: &[u8] = include_bytes!("../../hardware/som_power_share/SoM_Power_Share.py");
    const GOLDEN: &str = include_str!("../../hardware/som_power_share/golden.json");

    /// Plays back the transfers recorded in one golden case.
    struct Replay {
        set_page: bool,
        vout_mode: Option<u8>,
        words: Mutex<[VecDeque<Option<u16>>; 3]>,
        page: Mutex<Option<u8>>,
    }

    impl PowerI2cBackend for Replay {
        fn set_page(&self, _bus: i32, _addr: u8, page: u8) -> Result<(), String> {
            *self.page.lock().unwrap() = Some(page);
            self.set_page
                .then_some(())
                .ok_or_else(|| "page failed".into())
        }

        fn read_register(&self, _bus: i32, _addr: u8, register: u8) -> Result<u8, String> {
            assert_eq!(register, REG_VOUT_MODE);
            self.vout_mode.ok_or_else(|| "VOUT_MODE failed".into())
        }

        fn read_word(&self, _bus: i32, _addr: u8, register: u8) -> Result<u16, String> {
            let slot = [REG_POUT, REG_VOUT, REG_IOUT]
                .iter()
                .position(|reg| *reg == register)
                .expect("script reads only POUT, VOUT and IOUT words");
            self.words.lock().unwrap()[slot]
                .pop_front()
                .expect("script reads SAMPLES words per register")
                .ok_or_else(|| "word failed".into())
        }
    }

    fn close(actual: Option<f64>, expected: &Value) -> bool {
        match (actual, expected.as_f64()) {
            (None, None) => expected.is_null(),
            (Some(a), Some(e)) => (a - e).abs() <= 1e-12 * e.abs().max(1.0),
            _ => false,
        }
    }

    #[test]
    fn vendored_script_is_the_pinned_version() {
        let digest: String = Sha256::digest(SCRIPT)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(digest, SCRIPT_SHA256);
    }

    #[test]
    fn port_matches_the_script_on_every_golden_case() {
        let golden: Value = serde_json::from_str(GOLDEN).unwrap();
        assert_eq!(golden["script_sha256"], SCRIPT_SHA256);
        let cases = golden["cases"].as_array().unwrap();
        assert!(cases.len() >= 30);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let pmic = PMICS
                .iter()
                .find(|pmic| pmic.key == case["pmic"].as_str().unwrap())
                .unwrap();
            let rail = case["rail"].as_str().unwrap();
            let mut words: [VecDeque<Option<u16>>; 3] = Default::default();
            for sample in case["samples"].as_array().unwrap() {
                for (slot, word) in sample.as_array().unwrap().iter().enumerate() {
                    words[slot].push_back(word.as_u64().map(|word| word as u16));
                }
            }
            let replay = Replay {
                set_page: case["set_page"].as_bool().unwrap(),
                vout_mode: case["vout_mode"].as_u64().map(|mode| mode as u8),
                words: Mutex::new(words),
                page: Mutex::new(None),
            };

            let reading = read_rail(&replay, pmic, rail);

            let expected = &case["expected"];
            assert!(close(reading.vout, &expected["vout"]), "{name}: vout");
            assert!(close(reading.iout, &expected["iout"]), "{name}: iout");
            assert!(close(reading.pout, &expected["pout"]), "{name}: pout");
            let page = replay.page.lock().unwrap().map(u64::from);
            assert_eq!(page, case["page"].as_u64(), "{name}: page");
            if replay.set_page {
                let words = replay.words.lock().unwrap();
                assert!(words.iter().all(VecDeque::is_empty), "{name}: words left");
            }
        }
    }

    #[test]
    fn rail_lookup_follows_the_script_map() {
        let (pmic, rail) = find_rail(0x4f, 0).unwrap();
        assert_eq!((pmic.key, rail, pmic.core_rail), ("2", "0.75", "0.75"));
        let (pmic, rail) = find_rail(0x4d, 3).unwrap();
        assert_eq!((pmic.key, rail), ("1", "3.3"));
        assert!(find_rail(0x4d, 4).is_none());
        assert!(find_rail(0x40, 0).is_none());
    }
}
