//! Board-block tests. Every fixture is synthetic: sysfs trees, overlay blobs
//! (built here), and `fw_printenv` stand-ins (`sh -c`). The DevKit layout
//! (IMX477 behind an `i2c-mux-gpio` channel at `5-001a`, overlay names) is
//! transcribed from a Modalix DevKit and from the overlays the SDK ships, but
//! none of the files are captures.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use super::*;
use crate::peripherals::camera::{Camera, Isp, MipiCamera};
use crate::peripherals::sysutil::testing::{write_file, TempDir};
use crate::peripherals::{Availability, AvailabilityState};

const MODEL: &str = "SiMa.ai Modalix SoM 16Gig Board";
const OVERLAY: &str = "modalix-som-waveshare-ARDU-IMX477-1CAM.dtbo";
const SENSOR_NODE: &str = "i2cmux@0/i2c@0/imx477@1a";
/// u-boot-tools' `fw_printenv` when `dtbos` is not set.
const NOT_DEFINED: &str = "echo '## Error: \"dtbos\" not defined' >&2; exit 1";

/// A flattened-device-tree writer (version 17, big-endian).
#[derive(Default)]
struct Fdt {
    structure: Vec<u8>,
    strings: Vec<u8>,
}

impl Fdt {
    fn word(&mut self, value: u32) {
        self.structure.extend_from_slice(&value.to_be_bytes());
    }

    fn pad(&mut self) {
        while self.structure.len() & 3 != 0 {
            self.structure.push(0);
        }
    }

    fn begin(&mut self, name: &str) -> &mut Self {
        self.word(1);
        self.structure.extend_from_slice(name.as_bytes());
        self.structure.push(0);
        self.pad();
        self
    }

    fn property(&mut self, name: &str, value: &[u8]) -> &mut Self {
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(name.as_bytes());
        self.strings.push(0);
        self.word(3);
        self.word(value.len() as u32);
        self.word(offset);
        self.structure.extend_from_slice(value);
        self.pad();
        self
    }

    fn end(&mut self) -> &mut Self {
        self.word(2);
        self
    }

    fn build(&mut self) -> Vec<u8> {
        self.word(9);
        let struct_offset = 40 + 16;
        let strings_offset = struct_offset + self.structure.len();
        let total = strings_offset + self.strings.len();
        let header = [
            0xd00d_feed,
            total as u32,
            struct_offset as u32,
            strings_offset as u32,
            40,
            17,
            16,
            0,
            self.strings.len() as u32,
            self.structure.len() as u32,
        ];
        let mut blob: Vec<u8> = header.iter().flat_map(|word| word.to_be_bytes()).collect();
        blob.extend_from_slice(&[0; 16]);
        blob.extend_from_slice(&self.structure);
        blob.extend_from_slice(&self.strings);
        blob
    }
}

fn cells(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect()
}

fn text(value: &str) -> Vec<u8> {
    format!("{value}\0").into_bytes()
}

/// `name { compatible; port@1 { endpoint@0 { data-lanes } } }`, left open.
fn sensor<'a>(fdt: &'a mut Fdt, name: &str, compatible: &str, lanes: &[u32]) -> &'a mut Fdt {
    fdt.begin(name).property("compatible", &text(compatible));
    fdt.begin("port@1").begin("endpoint@0");
    fdt.property("data-lanes", &cells(lanes)).end().end()
}

/// The DevKit's IMX477 overlay: a GPIO I2C mux added at the root
/// (`target-path = "/"`) with the sensor on its first channel.
fn mux_overlay(compatible: &str) -> Vec<u8> {
    let mut fdt = Fdt::default();
    fdt.begin("").begin("fragment@0");
    fdt.property("target-path", &text("/")).begin("__overlay__");
    fdt.begin("i2cmux@0")
        .property("compatible", &text("i2c-mux-gpio"))
        .begin("i2c@0");
    sensor(&mut fdt, "imx477@1a", compatible, &[1, 2]).end();
    fdt.end().end().end().end();
    // The CSI receiver's endpoint gets data-lanes too; it is no device.
    fdt.begin("fragment@1")
        .property("target", &cells(&[0xffff_ffff]));
    fdt.begin("__overlay__")
        .property("data-lanes", &cells(&[1, 2]))
        .end()
        .end();
    fdt.begin("__fixups__")
        .property("csi10in", &text("/fragment@1:target:0"))
        .end();
    fdt.end().build()
}

/// The DVT layout: the sensor is added straight to an I2C bus that the
/// fragment's `target` phandle names, resolved through `__fixups__`.
fn fixup_overlay(compatible: &str, label: &str) -> Vec<u8> {
    let mut fdt = Fdt::default();
    fdt.begin("").begin("fragment@7");
    fdt.property("target", &cells(&[0xffff_ffff]))
        .begin("__overlay__");
    sensor(&mut fdt, "imx219@10", compatible, &[1, 2]).end();
    fdt.end().end();
    fdt.begin("__fixups__")
        .property(label, &text("/fragment@7:target:0"))
        .end();
    fdt.end().build()
}

pub(crate) fn mipi_camera(name: &str) -> Peripheral {
    Peripheral::Camera(Camera {
        id: format!("camera:{name}"),
        model: None,
        availability: Availability {
            state: AvailabilityState::Unknown,
            reason: None,
            subdevices: None,
            subdevices_available: None,
        },
        modes: Vec::new(),
        source: Source::Mipi(MipiCamera {
            camera_name: name.into(),
            media_device: "/dev/media0".into(),
            bus_info: None,
            isp: Isp::Unavailable {
                reason: "test".into(),
            },
            csi_receiver: None,
            sensor_timing: None,
            max_fps: None,
        }),
    })
}

/// A probe of `root/sys` and `root/boot` whose overlay list is printed by
/// `sh -c script`.
fn probe(root: &Path, script: &str) -> BoardProbe {
    let mut probe = BoardProbe::with_roots(root.join("sys"), root.join("boot"));
    probe.overlay_command = ["sh", "-c", script].map(String::from).to_vec();
    probe
}

fn dt_base(root: &Path) -> std::path::PathBuf {
    root.join("sys/firmware/devicetree/base")
}

/// A live device-tree node with the given properties.
fn dt_node(root: &Path, node: &str, properties: &[(&str, Vec<u8>)]) {
    let directory = dt_base(root).join(node);
    fs::create_dir_all(&directory).unwrap();
    for (name, value) in properties {
        fs::write(directory.join(name), value).unwrap();
    }
}

/// The live IMX477 node with its endpoint.
fn dt_sensor(root: &Path, node: &str, compatible: &str, lanes: &[u32]) {
    dt_node(root, node, &[("compatible", text(compatible))]);
    let endpoint = format!("{node}/port@1/endpoint@0");
    dt_node(root, &endpoint, &[("data-lanes", cells(lanes))]);
}

/// `/sys/bus/i2c/devices/<name>`, linked to its device directory, whose
/// `of_node` links to the live node `node`.
fn i2c_device(root: &Path, name: &str, node: Option<&str>) {
    let device = root.join("sys/devices/platform/i2c").join(name);
    fs::create_dir_all(&device).unwrap();
    if let Some(node) = node {
        symlink(dt_base(root).join(node), device.join("of_node")).unwrap();
    }
    let devices = root.join("sys/bus/i2c/devices");
    fs::create_dir_all(&devices).unwrap();
    symlink(&device, devices.join(name)).unwrap();
}

/// The DevKit with the IMX477 configured, plus I2C entries that are not
/// cameras, and the overlay in both boot slots.
fn devkit(root: &Path) {
    dt_node(root, "", &[("model", text(MODEL))]);
    dt_node(root, "i2cmux@0", &[("compatible", text("i2c-mux-gpio"))]);
    dt_sensor(root, SENSOR_NODE, "sony,imx477", &[1, 2]);
    dt_node(
        root,
        "i2c@40",
        &[("compatible", text("snps,designware-i2c"))],
    );
    dt_node(
        root,
        "i2c@40/eeprom@50",
        &[("compatible", text("atmel,24c32"))],
    );
    i2c_device(root, "5-001a", Some(SENSOR_NODE));
    // The mux channel is an adapter whose node holds the sensor's endpoint.
    i2c_device(root, "i2c-5", Some("i2cmux@0/i2c@0"));
    i2c_device(root, "0-0050", Some("i2c@40/eeprom@50"));
    i2c_device(root, "1-0068", None);
    for slot in ["boot-0", "boot-1"] {
        let path = root.join("boot").join(slot).join(OVERLAY);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, mux_overlay("sony,imx477")).unwrap();
    }
}

fn scan(probe: &mut BoardProbe, devices: &[Peripheral]) -> (serde_json::Value, Vec<CatalogError>) {
    let (board, errors) = probe.scan(devices);
    let value = serde_json::to_value(&board).unwrap();
    assert_eq!(
        serde_json::from_value::<Board>(value.clone()).unwrap(),
        board
    );
    (value, errors)
}

#[test]
fn a_configured_camera_names_the_catalog_camera_that_is_its_sensor() {
    let root = TempDir::new();
    devkit(root.path());
    let mut probe = probe(root.path(), &format!("echo '{OVERLAY}'"));
    let devices = [mipi_camera("imx477 5-001a"), mipi_camera("imx219 6-0010")];
    let (board, errors) = scan(&mut probe, &devices);
    assert_eq!(errors, []);
    assert_eq!(
        board,
        json!({
            "model": MODEL,
            "overlays": [OVERLAY],
            "configured_cameras": [{
                "compatible": "sony,imx477",
                "dt_node": "/i2cmux@0/i2c@0/imx477@1a",
                "i2c_device": "5-001a",
                "data_lanes": 2,
                "camera_id": "camera:imx477 5-001a"
            }],
            "supported_sensors": [{"compatible": "sony,imx477", "overlays": [OVERLAY]}]
        })
    );
}

#[test]
fn an_of_node_that_cannot_be_resolved_is_reported() {
    // A failure other than a vanished device must not read as "no configured
    // camera".
    let root = TempDir::new();
    devkit(root.path());
    let device = root.path().join("sys/devices/platform/i2c/9-0010");
    fs::create_dir_all(&device).unwrap();
    symlink(device.join("of_node"), device.join("of_node")).unwrap();
    symlink(&device, root.path().join("sys/bus/i2c/devices/9-0010")).unwrap();
    let mut probe = probe(root.path(), "true");
    let (board, errors) = scan(&mut probe, &[mipi_camera("imx477 5-001a")]);
    // The resolvable camera is still listed; the loop is one board error.
    assert_eq!(board["configured_cameras"][0]["i2c_device"], "5-001a");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].provider, "board.configured_cameras");
    assert!(
        errors[0].reason.contains("failed to resolve"),
        "{}",
        errors[0].reason
    );
}

#[test]
fn a_configured_camera_without_a_catalog_camera_has_no_camera_id() {
    let root = TempDir::new();
    devkit(root.path());
    dt_sensor(
        root.path(),
        "i2cmux@0/i2c@1/imx477@1a",
        "sony,imx477",
        &[1, 2, 3, 4],
    );
    i2c_device(root.path(), "6-001a", Some("i2cmux@0/i2c@1/imx477@1a"));
    let mut probe = probe(root.path(), "true");
    // A camera on another bus, and one whose name only starts with the device.
    let devices = [mipi_camera("imx477 7-001a"), mipi_camera("imx477 5-001a0")];
    let (board, errors) = scan(&mut probe, &devices);
    assert_eq!(errors, []);
    assert_eq!(
        board["configured_cameras"],
        json!([
            {"compatible": "sony,imx477", "dt_node": "/i2cmux@0/i2c@0/imx477@1a",
             "i2c_device": "5-001a", "data_lanes": 2},
            {"compatible": "sony,imx477", "dt_node": "/i2cmux@0/i2c@1/imx477@1a",
             "i2c_device": "6-001a", "data_lanes": 4}
        ])
    );
    // `true` prints nothing: the variable is set but holds no overlay.
    assert_eq!(board["overlays"], json!([]));
}

/// V4L2 may add a word after the I2C device (`ccs 5-0010 pixel_array`); a
/// longer address that starts with the device is another device.
#[test]
fn camera_id_matches_the_i2c_device_as_a_whole_word() {
    let root = TempDir::new();
    let node = "i2c@40/ccs@10";
    dt_sensor(root.path(), node, "mipi-ccs-1.1", &[1, 2]);
    i2c_device(root.path(), "5-0010", Some(node));
    let mut probe = probe(root.path(), "exit 0");
    let near_miss = [mipi_camera("ccs 5-00100 pixel_array")];
    let (board, _) = scan(&mut probe, &near_miss);
    assert!(
        board["configured_cameras"][0].get("camera_id").is_none(),
        "{board}"
    );
    let devices = [
        mipi_camera("ccs 5-00100"),
        mipi_camera("ccs 5-0010 pixel_array"),
    ];
    let (board, errors) = scan(&mut probe, &devices);
    assert_eq!(errors, []);
    assert_eq!(
        board["configured_cameras"][0]["camera_id"],
        "camera:ccs 5-0010 pixel_array"
    );
}

/// `dtbos` holds every overlay U-Boot applies, not only camera overlays.
#[test]
fn overlays_lists_every_dtbos_entry_in_order() {
    let root = TempDir::new();
    let mut probe = probe(root.path(), "echo 'pcie-8rc.dtbo a-imx477.dtbo'");
    let (board, errors) = scan(&mut probe, &[]);
    assert_eq!(errors, []);
    assert_eq!(board["overlays"], json!(["pcie-8rc.dtbo", "a-imx477.dtbo"]));
}

/// A GMSL link: deserializer, serializer, and sensor are all I2C devices
/// with CSI-2 endpoints; only the sensor, the innermost, is a camera.
#[test]
fn bridges_in_front_of_a_sensor_are_not_cameras() {
    let root = TempDir::new();
    let deserializer = "i2c@30/gmsl-deserializer@28";
    let serializer = format!("{deserializer}/i2c-atr/i2c@0/gmsl-serializer@42");
    let sensor = format!("{serializer}/i2c-atr/i2c@0/imx477@1a");
    dt_sensor(root.path(), deserializer, "maxim,max96716a", &[1, 2, 3, 4]);
    dt_sensor(root.path(), &serializer, "maxim,max96717", &[1, 2, 3, 4]);
    dt_sensor(root.path(), &sensor, "sony,imx477", &[1, 2, 3, 4]);
    i2c_device(root.path(), "3-0028", Some(deserializer));
    i2c_device(root.path(), "3-0042", Some(&serializer));
    i2c_device(root.path(), "9-001a", Some(&sensor));
    let (board, errors) = scan(&mut probe(root.path(), NOT_DEFINED), &[]);
    assert_eq!(errors, []);
    let cameras = board["configured_cameras"].as_array().unwrap();
    assert_eq!(cameras.len(), 1, "{board}");
    assert_eq!(cameras[0]["i2c_device"], "9-001a");
    assert_eq!(cameras[0]["compatible"], "sony,imx477");
}

#[test]
fn a_board_without_a_device_tree_reports_no_facts_and_no_errors() {
    let root = TempDir::new();
    let mut probe = probe(root.path(), NOT_DEFINED);
    let (board, errors) = scan(&mut probe, &[]);
    assert_eq!(errors, []);
    assert_eq!(
        board,
        json!({"configured_cameras": [], "supported_sensors": []})
    );
    // I2C devices without device-tree nodes (ACPI, user-instantiated).
    i2c_device(root.path(), "0-0050", None);
    assert_eq!(scan(&mut probe, &[]), (board, Vec::new()));
}

#[test]
fn the_overlay_list_keeps_dtbo_entries_and_degrades_without_fw_printenv() {
    let root = TempDir::new();
    let list = |command: &[&str], timeout| {
        let command: Vec<String> = command.iter().map(|part| part.to_string()).collect();
        overlay_list(&command, timeout)
    };
    let second = Duration::from_secs(2);
    let printed = list(
        &["sh", "-c", "printf ' a.dtbo  b.dtb\\n\\tc.dtbo\\n'"],
        second,
    );
    assert_eq!(printed, Ok(Some(vec!["a.dtbo".into(), "c.dtbo".into()])));
    // Missing tool, and a variable that is not set: no list, no error.
    let missing = root.path().join("fw_printenv");
    assert_eq!(list(&[missing.to_str().unwrap()], second), Ok(None));
    let not_defined = "echo a.dtbo; echo '## Error: \"dtbos\" not defined' >&2; exit 1";
    assert_eq!(list(&["sh", "-c", not_defined], second), Ok(None));
    // Any other failure: no list, and an error naming the first line.
    let failed =
        "echo a.dtbo; printf '\\nCannot read environment, using default\\nmore\\n' >&2; exit 1";
    let failed = list(&["sh", "-c", failed], second).unwrap_err();
    assert_eq!(failed.code, CODE_DISCOVERY_FAILED);
    assert_eq!(
        failed.reason,
        "sh failed (exit status: 1): Cannot read environment, using default"
    );
    let silent = list(&["sh", "-c", "exit 3"], second).unwrap_err();
    assert_eq!(silent.reason, "sh failed (exit status: 3)");
    // Present but not executable.
    write_file(&missing, "#!/bin/sh\n");
    let denied = list(&[missing.to_str().unwrap()], second).unwrap_err();
    assert_eq!(denied.code, CODE_PERMISSION_DENIED);
    // Output larger than a pipe holds is read while the tool runs, kept up to
    // the limit, and does not hold the tool past the timeout.
    let started = Instant::now();
    let flood =
        "head -c 300000 /dev/zero | tr '\\0' ' '; echo a.dtbo; head -c 300000 /dev/zero >&2";
    assert_eq!(list(&["sh", "-c", flood], second), Ok(Some(Vec::new())));
    assert!(started.elapsed() < second, "{:?}", started.elapsed());
    let tail = "printf 'a.dtbo '; head -c 300000 /dev/zero | tr '\\0' ' '";
    assert_eq!(
        list(&["sh", "-c", tail], second),
        Ok(Some(vec!["a.dtbo".into()]))
    );
    // A hung tool is killed at the timeout.
    let started = Instant::now();
    let hung = list(&["sh", "-c", "exec sleep 5"], Duration::from_millis(200)).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(hung.code, CODE_DISCOVERY_FAILED);
    assert!(hung.reason.contains("did not finish"), "{}", hung.reason);
}

#[test]
fn a_child_that_outlives_the_kill_grace_is_reaped_in_the_background() {
    // SIGKILL waits for uninterruptible I/O, which a test cannot cause; a child
    // that is not killed stands in for one that has not died yet.
    let child = Command::new("sleep").arg("1").spawn().unwrap();
    let pid = child.id();
    let started = Instant::now();
    assert!(!reap_within(child, Duration::from_millis(100)));
    assert!(started.elapsed() < Duration::from_millis(900));
    // The reaper waits for it, so it does not stay a zombie.
    let proc = Path::new("/proc").join(pid.to_string());
    let deadline = Instant::now() + Duration::from_secs(5);
    while proc.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!proc.exists(), "process {pid} was not reaped");
    // A child that exits within the grace is reaped in place.
    let mut child = Command::new("true").spawn().unwrap();
    let _ = child.kill();
    assert!(reap_within(child, Duration::from_secs(2)));
}

#[test]
fn a_pipe_that_cannot_be_made_non_blocking_is_an_error() {
    /// A descriptor number no test process has open.
    struct Closed;
    impl AsRawFd for Closed {
        fn as_raw_fd(&self) -> std::os::fd::RawFd {
            1 << 20
        }
    }
    let error = set_nonblocking(Some(&Closed)).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EBADF));
    assert!(set_nonblocking(None::<&Closed>).is_ok());
}

#[test]
fn a_hung_fw_printenv_omits_the_list_and_reports_one_error() {
    let root = TempDir::new();
    devkit(root.path());
    let mut probe = probe(root.path(), "exec sleep 5");
    probe.timeout = Duration::from_millis(100);
    let (board, errors) = scan(&mut probe, &[]);
    assert!(board.get("overlays").is_none(), "{board}");
    assert_eq!(board["model"], MODEL);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].provider, "board.overlays");
    assert_eq!(errors[0].code, CODE_DISCOVERY_FAILED);
}

#[test]
fn a_failed_fw_printenv_keeps_the_last_overlay_list() {
    let root = TempDir::new();
    devkit(root.path());
    let mut probe = probe(root.path(), "echo a.dtbo");
    let mut overlays = |script: &str| {
        probe.overlay_command[2] = script.into();
        let (board, errors) = scan(&mut probe, &[]);
        let providers: Vec<_> = errors.into_iter().map(|error| error.provider).collect();
        (board.get("overlays").cloned(), providers)
    };
    let failed = "echo 'Cannot read environment, using default' >&2; exit 1";
    assert_eq!(overlays("echo a.dtbo"), (Some(json!(["a.dtbo"])), vec![]));
    // The failure is reported, and the last list is kept.
    let kept = (Some(json!(["a.dtbo"])), vec!["board.overlays".to_string()]);
    assert_eq!(overlays(failed), kept);
    assert_eq!(overlays("echo b.dtbo"), (Some(json!(["b.dtbo"])), vec![]));
    // A variable that is not set is a result too: no list, then none kept.
    assert_eq!(overlays(NOT_DEFINED), (None, vec![]));
    let none = (None, vec!["board.overlays".to_string()]);
    assert_eq!(overlays(failed), none);
}

#[test]
fn a_failing_fw_printenv_reports_one_error_unless_the_variable_is_not_defined() {
    let root = TempDir::new();
    devkit(root.path());
    let (board, errors) = scan(&mut probe(root.path(), NOT_DEFINED), &[]);
    assert_eq!(errors, []);
    assert!(board.get("overlays").is_none(), "{board}");
    let unreadable = "echo 'Cannot read environment, using default' >&2; exit 1";
    let (board, errors) = scan(&mut probe(root.path(), unreadable), &[]);
    assert!(board.get("overlays").is_none(), "{board}");
    assert_eq!(board["model"], MODEL);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].provider, "board.overlays");
    assert_eq!(errors[0].code, CODE_DISCOVERY_FAILED);
    assert!(
        errors[0]
            .reason
            .ends_with(": Cannot read environment, using default"),
        "{}",
        errors[0].reason
    );
}

#[test]
fn overlay_sensors_are_the_innermost_csi2_sources_on_an_i2c_bus() {
    let tree = |blob: Vec<u8>| dt::parse_fdt(&blob).unwrap();
    assert_eq!(
        dt::overlay_sensors(&tree(mux_overlay("sony,imx477"))),
        ["sony,imx477"]
    );
    assert_eq!(
        dt::overlay_sensors(&tree(fixup_overlay("sony,imx219", "i2c01"))),
        ["sony,imx219"]
    );
    // A fragment whose target is not an I2C bus adds no sensor.
    assert!(dt::overlay_sensors(&tree(fixup_overlay("sony,imx219", "csi1"))).is_empty());
    // A device with a CSI-2 endpoint that is on no I2C bus.
    let mut fdt = Fdt::default();
    fdt.begin("").begin("fragment@0");
    fdt.property("target-path", &text("/soc"))
        .begin("__overlay__");
    sensor(&mut fdt, "bridge@0", "vendor,csi-bridge", &[1, 2]).end();
    fdt.end().end().end();
    assert!(dt::overlay_sensors(&tree(fdt.build())).is_empty());
    // `target-path` naming an I2C bus.
    let mut fdt = Fdt::default();
    fdt.begin("").begin("fragment@0");
    fdt.property("target-path", &text("/soc/i2c@4"))
        .begin("__overlay__");
    sensor(&mut fdt, "ar0234@10", "onsemi,ar0234", &[1, 2]).end();
    // A second sensor behind an I2C mux, which is not a sensor itself.
    fdt.begin("i2c-mux@70")
        .property("compatible", &text("nxp,pca9548"))
        .begin("i2c@0");
    sensor(&mut fdt, "imx477@1a", "sony,imx477", &[1, 2]).end();
    fdt.end().end().end().end().end();
    let sensors = dt::overlay_sensors(&tree(fdt.build()));
    assert_eq!(sensors, ["onsemi,ar0234", "sony,imx477"]);
}

#[test]
fn the_fdt_parser_rejects_malformed_blobs() {
    let good = mux_overlay("sony,imx477");
    assert!(dt::parse_fdt(&good).is_ok());
    let reason = |blob: &[u8]| dt::parse_fdt(blob).err().unwrap();
    assert!(reason(&good[..good.len() - 8]).contains("truncated"));
    assert!(reason(&good[..20]).contains("truncated"));
    assert!(reason(b"").contains("truncated"));
    assert!(reason(&[0u8; 64]).contains("bad magic"));
    // The structure block ends before the tree does.
    let mut cut = good.clone();
    cut[36..40].copy_from_slice(&16u32.to_be_bytes());
    assert!(dt::parse_fdt(&cut).is_err());
    // An unknown token.
    let mut fdt = Fdt::default();
    fdt.begin("").word(7);
    assert!(reason(&fdt.build()).contains("unknown token"));
    // An unclosed node.
    let mut fdt = Fdt::default();
    fdt.begin("").begin("a");
    assert!(reason(&fdt.build()).contains("ended inside a node"));
    // A property name offset outside the strings block.
    let mut fdt = Fdt::default();
    fdt.begin("").property("compatible", b"x\0");
    let mut blob = fdt.end().build();
    blob[56 + 16..56 + 20].copy_from_slice(&999u32.to_be_bytes());
    assert!(dt::parse_fdt(&blob).is_err());
    // Nesting deeper than the bound.
    let mut fdt = Fdt::default();
    for _ in 0..40 {
        fdt.begin("n");
    }
    assert!(reason(&fdt.build()).contains("deeper"));
}

#[test]
fn overlays_are_deduplicated_and_malformed_ones_are_skipped_with_one_error() {
    let root = TempDir::new();
    let write = |slot: &str, name: &str, blob: &[u8]| {
        let path = root.path().join("boot").join(slot).join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, blob).unwrap();
    };
    write("boot-0", "a-imx477.dtbo", &mux_overlay("sony,imx477"));
    write("boot-1", "a-imx477.dtbo", &mux_overlay("sony,imx477"));
    write(
        "boot-1",
        "b-imx219.dtbo",
        &fixup_overlay("sony,imx219", "i2c01"),
    );
    write("boot-0", "c-dual.dtbo", &mux_overlay("sony,imx477"));
    write(
        "boot-0",
        "pcie.dtbo",
        &fixup_overlay("sony,imx219", "pcie0"),
    );
    write("boot-0", "broken.dtbo", b"\xd0\x0d\xfe\xed");
    write("boot-1", "short.dtbo", &mux_overlay("sony,imx477")[..100]);
    // Not an overlay, and an overlay too deep to be read.
    write("boot-0", "board.dtb", &mux_overlay("sony,imx477"));
    write("boot-0/extra", "d.dtbo", &mux_overlay("sony,imx999"));
    fs::write(
        root.path().join("boot/top.dtbo"),
        mux_overlay("sony,imx999"),
    )
    .unwrap();
    let mut probe = probe(root.path(), NOT_DEFINED);
    let (board, errors) = scan(&mut probe, &[]);
    assert_eq!(
        board["supported_sensors"],
        json!([
            {"compatible": "sony,imx219", "overlays": ["b-imx219.dtbo"]},
            {"compatible": "sony,imx477", "overlays": ["a-imx477.dtbo", "c-dual.dtbo"]}
        ])
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].provider, "board.supported_sensors");
    assert_eq!(errors[0].code, CODE_DISCOVERY_FAILED);
    assert!(
        errors[0].reason.contains("broken.dtbo is malformed")
            && errors[0]
                .reason
                .ends_with("(and 1 more overlay files skipped)"),
        "{}",
        errors[0].reason
    );

    // A changed file is parsed again; an unchanged one comes from the cache.
    write(
        "boot-1",
        "short.dtbo",
        &fixup_overlay("sony,imx415", "i2c3"),
    );
    write(
        "boot-0",
        "broken.dtbo",
        &fixup_overlay("sony,imx415", "i2c3"),
    );
    let (board, errors) = scan(&mut probe, &[]);
    assert_eq!(errors, []);
    assert_eq!(
        board["supported_sensors"][1],
        json!({"compatible": "sony,imx415", "overlays": ["broken.dtbo", "short.dtbo"]})
    );
}

/// An unreadable overlay is not cached, so it parses again once it is
/// readable, without a restart.
#[test]
fn an_unreadable_overlay_is_read_again_when_it_becomes_readable() {
    let root = TempDir::new();
    let path = root.path().join("boot/boot-0/a-imx477.dtbo");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, mux_overlay("sony,imx477")).unwrap();
    let mut probe = probe_at(root.path());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let (denied_board, denied) = scan(&mut probe, &[]);
    // chmod changes the ctime too; the cache must not hold the error either.
    let cached = probe.overlays.contains_key(&path);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let (board, errors) = scan(&mut probe, &[]);
    assert_eq!(errors, []);
    assert_eq!(
        board["supported_sensors"],
        json!([{"compatible": "sony,imx477", "overlays": ["a-imx477.dtbo"]}])
    );
    if unsafe { libc::geteuid() } == 0 {
        return; // Root ignores the permission bits.
    }
    assert!(!cached);
    assert_eq!(denied_board["supported_sensors"], json!([]));
    assert_eq!(denied.len(), 1, "{denied:?}");
    assert_eq!(denied[0].code, CODE_PERMISSION_DENIED);
}

/// A rewrite that keeps the size and restores the mtime is still seen, by
/// its ctime in place and by its inode when the file is replaced.
#[test]
fn a_same_size_rewrite_with_the_mtime_restored_is_parsed_again() {
    let root = TempDir::new();
    let path = root.path().join("boot/boot-0/a.dtbo");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let blob = |compatible| fixup_overlay(compatible, "i2c01");
    assert_eq!(blob("sony,imx219").len(), blob("sony,imx415").len());
    fs::write(&path, blob("sony,imx219")).unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let mut probe = probe_at(root.path());
    let sensor = |probe: &mut BoardProbe| {
        let (board, errors) = scan(probe, &[]);
        assert_eq!(errors, []);
        board["supported_sensors"][0]["compatible"].clone()
    };
    assert_eq!(sensor(&mut probe), "sony,imx219");

    // In place: the ctime changes. Past the coarse clock's tick.
    std::thread::sleep(Duration::from_millis(20));
    fs::write(&path, blob("sony,imx415")).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    assert_eq!(sensor(&mut probe), "sony,imx415");

    // Replaced by rename: the inode changes.
    let replacement = root.path().join("boot/replacement");
    fs::write(&replacement, blob("sony,imx219")).unwrap();
    fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    fs::rename(&replacement, &path).unwrap();
    assert_eq!(sensor(&mut probe), "sony,imx219");
}

#[test]
fn overlay_files_and_sizes_are_capped() {
    let root = TempDir::new();
    let slot = root.path().join("boot/boot-0");
    fs::create_dir_all(&slot).unwrap();
    let blob = fixup_overlay("sony,imx219", "i2c01");
    for index in 0..=MAX_OVERLAY_FILES {
        fs::write(slot.join(format!("{index:04}.dtbo")), &blob).unwrap();
    }
    let mut probe = probe(root.path(), NOT_DEFINED);
    let (board, errors) = scan(&mut probe, &[]);
    let overlays = board["supported_sensors"][0]["overlays"]
        .as_array()
        .unwrap();
    assert_eq!(overlays.len(), MAX_OVERLAY_FILES);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].reason.starts_with("more than 512 overlay files"));

    let root = TempDir::new();
    let path = root.path().join("boot/boot-0/huge.dtbo");
    let mut huge = blob.clone();
    huge.resize(MAX_OVERLAY_BYTES as usize + 1, 0);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, huge).unwrap();
    let (board, errors) = scan(&mut probe_at(root.path()), &[]);
    assert_eq!(board["supported_sensors"], json!([]));
    assert!(errors[0].reason.contains("larger than 1048576 bytes"));
}

/// The one error of a scan of `root`, which must be `provider`'s and say
/// `expected`.
fn only_error_says(root: &Path, provider: &str, expected: &str) {
    let (_, errors) = scan(&mut probe_at(root), &[]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].provider, provider);
    assert_eq!(errors[0].code, CODE_DISCOVERY_FAILED);
    assert!(errors[0].reason.contains(expected), "{errors:?}");
}

#[test]
fn every_listing_and_subtree_cut_at_a_bound_is_reported() {
    // More I2C devices than are examined.
    let root = TempDir::new();
    devkit(root.path());
    let devices = root.path().join("sys/bus/i2c/devices");
    for bus in 0..MAX_I2C_DEVICES {
        fs::create_dir(devices.join(format!("i2c-{}", bus + 100))).unwrap();
    }
    only_error_says(
        root.path(),
        "board.configured_cameras",
        "more than 1024 I2C devices in ",
    );

    // More entries in /boot, and in one of its directories, than are listed.
    for directory in ["boot", "boot/boot-0"] {
        let root = TempDir::new();
        devkit(root.path());
        let directory = root.path().join(directory);
        for index in 0..MAX_BOOT_ENTRIES {
            fs::write(directory.join(format!("{index:04}.txt")), "").unwrap();
        }
        let expected = format!("more than 4096 entries in {}", directory.display());
        only_error_says(root.path(), "board.supported_sensors", &expected);
    }

    // A live node with more entries, a subtree with more nodes, and nodes
    // nested deeper than are read.
    let root = TempDir::new();
    devkit(root.path());
    let sensor = dt_base(root.path()).join(SENSOR_NODE);
    for index in 0..256 {
        fs::write(sensor.join(format!("property-{index:03}")), "").unwrap();
    }
    only_error_says(
        root.path(),
        "board.configured_cameras",
        "more than 256 entries in ",
    );

    // 16 groups of 16 keep every node under the entry bound.
    let root = TempDir::new();
    devkit(root.path());
    let sensor = dt_base(root.path()).join(SENSOR_NODE);
    for group in 0..16 {
        for index in 0..16 {
            fs::create_dir_all(sensor.join(format!("g@{group}/n@{index}"))).unwrap();
        }
    }
    only_error_says(
        root.path(),
        "board.configured_cameras",
        "more than 256 device-tree nodes in ",
    );

    // The sensor is level 1; its 32nd descendant level is not read.
    let root = TempDir::new();
    devkit(root.path());
    let chain = |levels| {
        (0..levels).fold(dt_base(root.path()).join(SENSOR_NODE), |path, level| {
            path.join(format!("n{level}"))
        })
    };
    fs::create_dir_all(chain(32)).unwrap();
    only_error_says(
        root.path(),
        "board.configured_cameras",
        "n31 is nested deeper than 32 nodes and was not read",
    );

    // At each bound exactly, nothing is cut and nothing is reported: 32
    // levels, then 256 nodes (the sensor, its port and endpoint, and 253) in
    // 255 entries.
    fs::remove_dir(chain(32)).unwrap();
    let (board, errors) = scan(&mut probe_at(root.path()), &[]);
    assert_eq!(errors, [], "{board}");
    let root = TempDir::new();
    devkit(root.path());
    let sensor = dt_base(root.path()).join(SENSOR_NODE);
    for index in 0..253 {
        fs::create_dir(sensor.join(format!("node@{index}"))).unwrap();
    }
    let (board, errors) = scan(&mut probe_at(root.path()), &[]);
    assert_eq!(errors, [], "{board}");
    assert_eq!(board["configured_cameras"].as_array().unwrap().len(), 1);
}

fn probe_at(root: &Path) -> BoardProbe {
    probe(root, NOT_DEFINED)
}

#[test]
fn unreadable_sources_are_reported_and_the_rest_is_kept() {
    let root = TempDir::new();
    devkit(root.path());
    // A model that is a directory reads as EISDIR.
    fs::remove_file(dt_base(root.path()).join("model")).unwrap();
    fs::create_dir_all(dt_base(root.path()).join("model")).unwrap();
    let (board, errors) = scan(&mut probe_at(root.path()), &[]);
    assert!(board.get("model").is_none());
    assert_eq!(board["configured_cameras"].as_array().unwrap().len(), 1);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].provider, "board.model");
    assert_eq!(errors[0].code, "io.open");
    assert!(
        errors[0].reason.ends_with("model: Is a directory"),
        "{errors:?}"
    );
}

#[test]
fn i2c_client_names_are_bus_and_four_hex_digits() {
    assert!(is_i2c_client("5-001a"));
    assert!(is_i2c_client("12-a050"));
    assert!(!is_i2c_client("i2c-5"));
    assert!(!is_i2c_client("5-1a"));
    assert!(!is_i2c_client("-001a"));
    assert!(!is_i2c_client("5-001g"));
}

/// A probe over the fixture tree, for the worker's revision test.
pub(crate) const DEVKIT_MODEL: &str = MODEL;

pub(crate) fn devkit_probe(root: &Path) -> BoardProbe {
    devkit(root);
    probe(root, NOT_DEFINED)
}

pub(crate) fn set_model(root: &Path, model: &str) {
    fs::write(dt_base(root).join("model"), text(model)).unwrap();
}
