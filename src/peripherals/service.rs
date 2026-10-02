use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::catalog::{Catalog, DEFAULT_CHANGE_CAPACITY};
use super::model::{CatalogDocument, Provider};
use super::scan::{run_scan, ProviderSlot};
use super::uevent::UeventSocket;

pub const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Schedule {
    completed: u64,
    scanning: bool,
    refresh: bool,
    stop: bool,
}

/// Shared between the peripherals thread and its callers (API, daemon).
pub struct Control {
    schedule: Mutex<Schedule>,
    wake_read: OwnedFd,
    wake_write: OwnedFd,
}

impl Control {
    fn new() -> io::Result<Self> {
        let mut fds = [0; 2];
        // SAFETY: fds has room for the two descriptors pipe2 returns.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both descriptors were just created and are owned here.
        let (wake_read, wake_write) =
            unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok(Self {
            schedule: Mutex::new(Schedule::default()),
            wake_read,
            wake_write,
        })
    }

    /// Ask for a scan now. Returns the `scan_sequence` whose completion
    /// guarantees the result reflects the hardware as of this call.
    pub fn request_refresh(&self) -> u64 {
        let target = {
            let mut schedule = self.schedule.lock().unwrap_or_else(|e| e.into_inner());
            schedule.refresh = true;
            schedule.completed + if schedule.scanning { 2 } else { 1 }
        };
        self.wake();
        target
    }

    fn request_stop(&self) {
        self.schedule.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        self.wake();
    }

    fn wake(&self) {
        let byte = 1u8;
        // SAFETY: writing one byte from a live buffer. A full pipe already
        // guarantees a pending wake-up, so EAGAIN is ignored.
        unsafe {
            libc::write(
                self.wake_write.as_raw_fd(),
                &byte as *const u8 as *const libc::c_void,
                1,
            );
        }
    }

    fn drain_wake(&self) {
        let mut buffer = [0u8; 64];
        // SAFETY: reading into a live buffer from a non-blocking pipe.
        while unsafe {
            libc::read(
                self.wake_read.as_raw_fd(),
                buffer.as_mut_ptr() as *mut libc::c_void,
                buffer.len(),
            )
        } > 0
        {}
    }
}

pub struct Config {
    pub catalog_path: PathBuf,
    pub instance_id: String,
    pub debounce: Duration,
}

pub struct PeripheralsHandle {
    control: Arc<Control>,
    thread: JoinHandle<()>,
}

impl PeripheralsHandle {
    pub fn control(&self) -> Arc<Control> {
        self.control.clone()
    }

    pub fn stop(self) {
        self.control.request_stop();
        let _ = self.thread.join();
    }
}

/// Publish a `starting` catalog, then run the event loop on its own thread.
pub fn spawn(config: Config, providers: Vec<Box<dyn Provider>>) -> Result<PeripheralsHandle> {
    let control = Arc::new(Control::new().context("create peripherals wake pipe")?);
    let mut catalog = Catalog::new(config.instance_id.clone(), DEFAULT_CHANGE_CAPACITY);
    let subsystems: Vec<String> = providers
        .iter()
        .flat_map(|provider| provider.subsystems().iter().cloned())
        .collect();
    let uevents = match UeventSocket::open(subsystems) {
        Ok(socket) => Some(socket),
        Err(error) => {
            catalog.apply_error(
                "peripherals.monitor_failed",
                &format!(
                    "hot-plug events are unavailable; only explicit refreshes rescan: {error}"
                ),
            );
            None
        }
    };
    publish(&config.catalog_path, &catalog.document())?;
    let slots = providers.into_iter().map(ProviderSlot::new).collect();
    let thread_control = control.clone();
    let thread = thread::Builder::new()
        .name("peripherals".into())
        .spawn(move || run(config, thread_control, catalog, slots, uevents))
        .context("spawn peripherals thread")?;
    Ok(PeripheralsHandle { control, thread })
}

fn run(
    config: Config,
    control: Arc<Control>,
    mut catalog: Catalog,
    mut slots: Vec<ProviderSlot>,
    mut uevents: Option<UeventSocket>,
) {
    // The initial scan runs immediately; afterwards the thread sleeps in
    // poll() until a uevent, a refresh request, or shutdown.
    let mut due = Some(Instant::now());
    loop {
        let timeout = due.map_or(-1, |at| {
            at.saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as libc::c_int
        });
        let mut fds = [
            libc::pollfd {
                fd: control.wake_read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: uevents.as_ref().map_or(-1, UeventSocket::as_raw_fd),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is a valid array of two pollfd structures.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
        if ready < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            catalog.apply_error("peripherals.monitor_failed", "waiting for events failed");
            let _ = publish(&config.catalog_path, &catalog.document());
            return;
        }
        if fds[0].revents != 0 {
            control.drain_wake();
        }
        {
            let mut schedule = control.schedule.lock().unwrap_or_else(|e| e.into_inner());
            if schedule.stop {
                return;
            }
            if std::mem::take(&mut schedule.refresh) {
                due = Some(Instant::now());
            }
        }
        if fds[1].revents != 0 {
            if let Some(socket) = &uevents {
                match socket.drain() {
                    Ok(drained) if drained.matched || drained.overflowed => {
                        // Trailing debounce: a burst produces one scan.
                        due = Some(Instant::now() + config.debounce);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        catalog.apply_error(
                            "peripherals.monitor_failed",
                            &format!("hot-plug events stopped: {error}"),
                        );
                        uevents = None;
                        let _ = publish(&config.catalog_path, &catalog.document());
                    }
                }
            }
        }
        if due.is_some_and(|at| Instant::now() >= at) {
            due = None;
            control
                .schedule
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .scanning = true;
            if let Err(error) = run_scan(&mut slots, &mut catalog) {
                eprintln!("Sentinel peripheral scan rejected: {error}");
            }
            {
                let mut schedule = control.schedule.lock().unwrap_or_else(|e| e.into_inner());
                schedule.scanning = false;
                schedule.completed = catalog.scan_sequence();
            }
            if let Err(error) = publish(&config.catalog_path, &catalog.document()) {
                eprintln!("Sentinel peripheral catalog write failed: {error:#}");
            }
        }
    }
}

/// Replace the catalog file atomically so readers never see a partial write.
pub fn publish(path: &Path, document: &CatalogDocument) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("peripherals.json")
    ));
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        serde_json::to_writer(&mut file, document).context("serialize peripheral catalog")?;
        file.write_all(b"\n")?;
    }
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

pub fn read(path: &Path) -> Result<CatalogDocument> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))
}

/// A new identifier per daemon start, so clients can detect restarts.
pub fn new_instance_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/uuid")
        .map(|uuid| uuid.trim().to_string())
        .unwrap_or_else(|_| {
            format!(
                "{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripherals::model::{ProviderError, Record};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    struct Counting {
        calls: Arc<AtomicUsize>,
        subsystems: Vec<String>,
    }

    impl Provider for Counting {
        fn name(&self) -> &str {
            "test.camera"
        }
        fn subsystems(&self) -> &[String] {
            &self.subsystems
        }
        fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((0..=call)
                .map(|index| Record {
                    id: format!("camera:{index}"),
                    kind: "camera".into(),
                    provider: "test.camera".into(),
                    details: json!({}),
                })
                .collect())
        }
    }

    fn wait_for(path: &Path, predicate: impl Fn(&CatalogDocument) -> bool) -> CatalogDocument {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(document) = read(path) {
                if predicate(&document) {
                    return document;
                }
            }
            assert!(
                Instant::now() < deadline,
                "catalog never reached the expected state"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn publishes_initial_scan_and_honours_refresh_targets() {
        let root = std::env::temp_dir().join(format!(
            "sentinel-peripherals-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("peripherals.json");
        let calls = Arc::new(AtomicUsize::new(0));
        let handle = spawn(
            Config {
                catalog_path: path.clone(),
                instance_id: "test-instance".into(),
                debounce: Duration::from_millis(10),
            },
            vec![Box::new(Counting {
                calls: calls.clone(),
                subsystems: vec!["video4linux".into()],
            })],
        )
        .unwrap();

        let first = wait_for(&path, |document| document.ready);
        assert_eq!(first.instance_id, "test-instance");
        assert_eq!((first.revision, first.scan_sequence), (1, 1));
        assert_eq!(first.devices.len(), 1);

        let target = handle.control().request_refresh();
        assert_eq!(target, 2);
        let refreshed = wait_for(&path, |document| document.scan_sequence >= target);
        assert_eq!(refreshed.revision, 2);
        assert_eq!(refreshed.devices.len(), 2);
        assert_eq!(refreshed.changes.last().unwrap().kind, "added");

        handle.stop();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "no scan without a trigger");
        fs::remove_dir_all(root).unwrap();
    }
}
