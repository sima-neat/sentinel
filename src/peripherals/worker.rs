use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;

use super::uevent::UeventSocket;
use super::{Catalog, CatalogError, Peripheral, Provider};

/// How long to let a burst of uevents settle (a USB device with several
/// interfaces sends many) before scanning.
const COALESCE: Duration = Duration::from_millis(250);
/// Discovery runs this much nicer than the daemon's metrics sampling.
const NICE: libc::c_int = 10;

/// Shared by the worker and every [`Peripherals`] handle.
struct Shared {
    catalog: RwLock<Catalog>,
    /// eventfd that wakes the worker for a refresh or a stop.
    wake: OwnedFd,
    stopping: AtomicBool,
    running: AtomicBool,
}

/// Read access and refresh requests, for the API.
#[derive(Clone)]
pub struct Peripherals(Arc<Shared>);

impl Peripherals {
    /// The latest catalog, or `None` when the worker has stopped.
    pub fn catalog(&self) -> Option<Catalog> {
        let catalog = self
            .0
            .catalog
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        self.0
            .running
            .load(Ordering::Acquire)
            .then(|| catalog.clone())
    }

    /// Ask for a rescan; `false` when the worker has stopped.
    pub fn refresh(&self) -> bool {
        self.0.running.load(Ordering::Acquire) && wake(self.0.wake.as_raw_fd())
    }
}

/// The running worker thread, owned by the daemon.
pub struct Worker {
    shared: Arc<Shared>,
    thread: JoinHandle<()>,
}

impl Worker {
    pub fn handle(&self) -> Peripherals {
        Peripherals(self.shared.clone())
    }

    pub fn stop(self) {
        self.shared.stopping.store(true, Ordering::Release);
        wake(self.shared.wake.as_raw_fd());
        let _ = self.thread.join();
    }
}

/// Start the worker; it scans immediately, then on every relevant uevent
/// and refresh request.
pub fn start(providers: Vec<Box<dyn Provider>>) -> Result<Worker> {
    spawn(providers, true)
}

fn spawn(providers: Vec<Box<dyn Provider>>, listen_for_uevents: bool) -> Result<Worker> {
    // SAFETY: plain eventfd creation; the result is checked below.
    let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if raw < 0 {
        return Err(io::Error::last_os_error()).context("create peripherals wake eventfd");
    }
    let shared = Arc::new(Shared {
        catalog: RwLock::new(Catalog {
            revision: Utc::now().timestamp_millis().max(0) as u64,
            observed_at: None,
            devices: Vec::new(),
            errors: Vec::new(),
        }),
        // SAFETY: raw is a freshly created, owned descriptor.
        wake: unsafe { OwnedFd::from_raw_fd(raw) },
        stopping: AtomicBool::new(false),
        running: AtomicBool::new(true),
    });
    let worker_shared = shared.clone();
    let thread = thread::Builder::new()
        .name("peripherals".into())
        .spawn(move || run(&worker_shared, providers, listen_for_uevents))
        .context("spawn peripherals thread")?;
    Ok(Worker { shared, thread })
}

/// Marks the worker stopped when it returns or panics.
struct RunningGuard<'a>(&'a Shared);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
    }
}

fn run(shared: &Shared, mut providers: Vec<Box<dyn Provider>>, listen_for_uevents: bool) {
    let _guard = RunningGuard(shared);
    lower_priority();
    let mut hotplug_error = None;
    let mut uevents = None;
    if listen_for_uevents {
        let subsystems = providers.iter().flat_map(|provider| provider.subsystems());
        match UeventSocket::open(subsystems.copied().collect()) {
            Ok(socket) => uevents = Some(socket),
            Err(error) => hotplug_error = Some(hotplug_unavailable(error)),
        }
    }
    let mut last_good: Vec<Option<Vec<Peripheral>>> = vec![None; providers.len()];
    loop {
        let observed_at = Utc::now();
        let mut devices = Vec::new();
        let mut errors: Vec<CatalogError> = hotplug_error.iter().cloned().collect();
        for (provider, good) in providers.iter_mut().zip(&mut last_good) {
            match provider.discover() {
                Ok(found) => *good = Some(found),
                Err(error) => errors.push(CatalogError {
                    provider: provider.name().into(),
                    code: error.code,
                    reason: error.reason,
                }),
            }
            devices.extend(good.iter().flatten().cloned());
        }
        devices.sort_by(|left, right| left.id().cmp(right.id()));
        let mut catalog = shared
            .catalog
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if catalog.devices != devices || catalog.errors != errors {
            catalog.revision += 1;
            catalog.devices = devices;
            catalog.errors = errors;
        }
        catalog.observed_at = Some(observed_at);
        drop(catalog);
        if !wait(shared, &mut uevents, &mut hotplug_error) {
            return;
        }
    }
}

/// Block until a relevant uevent or a refresh, then let the burst settle.
/// Returns `false` on stop.
fn wait(
    shared: &Shared,
    uevents: &mut Option<UeventSocket>,
    hotplug_error: &mut Option<CatalogError>,
) -> bool {
    let wake_fd = shared.wake.as_raw_fd();
    loop {
        let socket_fd = uevents.as_ref().map(UeventSocket::as_raw_fd);
        let ready = poll(wake_fd, socket_fd, -1);
        if shared.stopping.load(Ordering::Acquire) {
            return false;
        }
        let mut triggered = drain_wake(wake_fd);
        if ready.1 {
            triggered |= drain_uevents(uevents, hotplug_error);
        }
        if triggered {
            break;
        }
    }
    // A refresh during the pause wakes it early; that scan serves it too.
    poll(wake_fd, None, COALESCE.as_millis() as libc::c_int);
    if shared.stopping.load(Ordering::Acquire) {
        return false;
    }
    drain_wake(wake_fd);
    drain_uevents(uevents, hotplug_error);
    true
}

/// Whether a watched subsystem changed; a broken socket falls back to
/// refresh-only discovery.
fn drain_uevents(
    uevents: &mut Option<UeventSocket>,
    hotplug_error: &mut Option<CatalogError>,
) -> bool {
    let Some(socket) = uevents else {
        return false;
    };
    match socket.drain() {
        Ok(triggered) => triggered,
        Err(error) => {
            *hotplug_error = Some(hotplug_unavailable(error));
            *uevents = None;
            true
        }
    }
}

fn hotplug_unavailable(error: io::Error) -> CatalogError {
    CatalogError {
        provider: "hotplug".into(),
        code: "hotplug.unavailable".into(),
        reason: format!("kernel uevents unavailable ({error}); rescans happen only on refresh"),
    }
}

/// Poll the wake eventfd and, if present, the uevent socket. Returns which
/// of them are readable.
fn poll(wake_fd: RawFd, socket_fd: Option<RawFd>, timeout_ms: libc::c_int) -> (bool, bool) {
    let entry = |fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let mut fds = [entry(wake_fd), entry(socket_fd.unwrap_or(-1))];
    // SAFETY: fds is valid for its length; poll ignores negative descriptors.
    let count = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    if count < 0 {
        return (false, false);
    }
    (fds[0].revents != 0, fds[1].revents != 0)
}

fn wake(fd: RawFd) -> bool {
    let one: u64 = 1;
    // SAFETY: writes 8 bytes from a valid u64 to an eventfd.
    unsafe { libc::write(fd, &one as *const u64 as *const libc::c_void, 8) == 8 }
}

fn drain_wake(fd: RawFd) -> bool {
    let mut count: u64 = 0;
    // SAFETY: reads 8 bytes into a valid u64; the eventfd is non-blocking.
    unsafe { libc::read(fd, &mut count as *mut u64 as *mut libc::c_void, 8) == 8 }
}

fn lower_priority() {
    // SAFETY: gettid has no arguments; get/setpriority target only this
    // thread. Raise niceness relative to the daemon's own, capped at 19.
    let result = unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        let current = libc::getpriority(libc::PRIO_PROCESS, tid);
        libc::setpriority(libc::PRIO_PROCESS, tid, (current + NICE).min(19))
    };
    if result != 0 {
        eprintln!(
            "Sentinel could not lower the peripherals thread priority: {}",
            io::Error::last_os_error()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Instant;

    use super::super::tests::device;
    use super::super::ProviderError;
    use super::*;

    type ScanResult = std::result::Result<Vec<Peripheral>, ProviderError>;

    /// Replays scripted scan results; the last one repeats.
    struct Scripted {
        results: Arc<Mutex<VecDeque<ScanResult>>>,
    }

    impl Provider for Scripted {
        fn name(&self) -> &'static str {
            "test.scripted"
        }
        fn subsystems(&self) -> &'static [&'static str] {
            &["test"]
        }
        fn discover(&mut self) -> ScanResult {
            let mut results = self.results.lock().unwrap();
            match results.len() {
                0 => panic!("scripted provider ran out of results"),
                1 => results[0].clone(),
                _ => results.pop_front().unwrap(),
            }
        }
    }

    fn failure() -> ScanResult {
        Err(ProviderError {
            code: "io.permission_denied".into(),
            reason: "denied".into(),
        })
    }

    fn wait_for(peripherals: &Peripherals, done: impl Fn(&Catalog) -> bool) -> Catalog {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(catalog) = peripherals.catalog().filter(|catalog| done(catalog)) {
                return catalog;
            }
            assert!(
                Instant::now() < deadline,
                "timed out: {:?}",
                peripherals.catalog()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn refresh_rescans_and_failures_keep_the_last_good_devices() {
        let results = Arc::new(Mutex::new(VecDeque::from([
            Ok(vec![device("test:b"), device("test:a")]),
            Ok(vec![device("test:b"), device("test:a")]),
            failure(),
            Ok(vec![device("test:a")]),
        ])));
        let worker = spawn(
            vec![Box::new(Scripted {
                results: results.clone(),
            })],
            false,
        )
        .unwrap();
        let peripherals = worker.handle();

        let first = wait_for(&peripherals, |catalog| catalog.observed_at.is_some());
        let ids: Vec<_> = first.devices.iter().map(Peripheral::id).collect();
        assert_eq!(ids, ["test:a", "test:b"]);

        // An unchanged rescan advances observed_at but keeps the revision.
        assert!(peripherals.refresh());
        let unchanged = wait_for(&peripherals, |catalog| {
            catalog.observed_at > first.observed_at
        });
        assert_eq!(unchanged.revision, first.revision);

        // A failed provider reports an error and keeps its last good devices.
        assert!(peripherals.refresh());
        let failed = wait_for(&peripherals, |catalog| !catalog.errors.is_empty());
        assert_eq!(failed.devices, first.devices);
        assert_eq!(failed.errors[0].provider, "test.scripted");
        assert_eq!(failed.errors[0].code, "io.permission_denied");
        assert_ne!(failed.revision, first.revision);

        // Recovery clears the error and publishes the new device list.
        assert!(peripherals.refresh());
        let recovered = wait_for(&peripherals, |catalog| catalog.devices.len() == 1);
        assert!(recovered.errors.is_empty());
        assert_ne!(recovered.revision, failed.revision);

        worker.stop();
        assert_eq!(peripherals.catalog(), None);
        assert!(!peripherals.refresh());
    }

    #[test]
    fn a_panicking_provider_stops_serving_the_catalog() {
        let worker = spawn(
            vec![Box::new(Scripted {
                results: Arc::new(Mutex::new(VecDeque::new())),
            })],
            false,
        )
        .unwrap();
        let peripherals = worker.handle();
        let deadline = Instant::now() + Duration::from_secs(5);
        while peripherals.catalog().is_some() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!peripherals.refresh());
        worker.stop();
    }
}
