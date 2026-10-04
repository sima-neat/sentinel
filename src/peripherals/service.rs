use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::catalog::{Catalog, DEFAULT_CHANGE_CAPACITY};
use super::model::{CatalogDocument, Provider};
use super::scan::Scanner;
use super::support::{RulesWatch, SupportStage};
use super::uevent::UeventSocket;

pub const DEBOUNCE: Duration = Duration::from_millis(250);
/// Explicit refreshes are available on the local world-writable API socket;
/// limit how often they can make the daemon enumerate hardware.
pub const REFRESH_COOLDOWN: Duration = Duration::from_secs(5);
/// Discovery yields to camera pipelines on a busy board. Scan threads inherit
/// this from the peripherals thread.
pub const NICE: libc::c_int = 10;
/// A steady stream of events cannot postpone a scan by more than this.
pub const MAX_EVENT_WAIT: Duration = Duration::from_secs(1);
/// How often a failed catalog write is retried.
pub const PUBLISH_RETRY: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Schedule {
    completed: u64,
    scanning: bool,
    refresh: bool,
    stop: bool,
    /// False once the thread has exited, so refreshes are refused instead of
    /// promising a scan that will never run.
    alive: bool,
    /// Why the last catalog write failed, until one succeeds. The file is
    /// behind meanwhile, so it must not be served as current.
    publish_error: Option<String>,
    /// Filesystem identity of the last catalog this daemon published. This
    /// binds API reads to that publication even in an attacker-writable
    /// parent directory where an older daemon-owned file could be replayed.
    publication: Option<Publication>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Publication {
    device: u64,
    inode: u64,
}

impl Publication {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

/// Shared between the peripherals thread and its callers (API, daemon).
pub struct Control {
    instance_id: String,
    schedule: Mutex<Schedule>,
    wake_read: OwnedFd,
    wake_write: OwnedFd,
}

impl Control {
    fn new(instance_id: String) -> io::Result<Self> {
        let mut fds = [0; 2];
        // SAFETY: fds has room for the two descriptors pipe2 returns.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both descriptors were just created and are owned here.
        let (wake_read, wake_write) =
            unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok(Self {
            instance_id,
            schedule: Mutex::new(Schedule::default()),
            wake_read,
            wake_write,
        })
    }

    /// The schedule, even if a thread panicked while holding it.
    fn schedule(&self) -> MutexGuard<'_, Schedule> {
        self.schedule.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ask for a scan now. Returns the `scan_sequence` whose completion
    /// guarantees the result reflects the hardware as of this call, or `None`
    /// if the peripherals thread is no longer running.
    pub fn request_refresh(&self) -> Option<u64> {
        let target = {
            let mut schedule = self.schedule();
            if !schedule.alive {
                return None;
            }
            schedule.refresh = true;
            schedule.completed + if schedule.scanning { 2 } else { 1 }
        };
        self.wake();
        Some(target)
    }

    /// False once the peripherals thread has exited, cleanly or by a panic.
    pub fn is_alive(&self) -> bool {
        self.schedule().alive
    }

    /// The `instance_id` of the catalog this thread publishes.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Why the catalog file is behind the catalog, while writes fail.
    pub fn publish_error(&self) -> Option<String> {
        self.schedule().publish_error.clone()
    }

    /// Whether an open file is the most recent catalog publication made by
    /// this daemon instance.
    pub fn owns_publication(&self, metadata: &fs::Metadata) -> bool {
        self.schedule().publication == Some(Publication::from_metadata(metadata))
    }

    fn request_stop(&self) {
        self.schedule().stop = true;
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
    pub support_rules_path: PathBuf,
    pub instance_id: String,
    pub debounce: Duration,
    pub refresh_cooldown: Duration,
    /// Tests turn this off so host hot-plug events cannot trigger scans.
    pub listen_for_uevents: bool,
    /// Tests may choose the temporary path to exercise write failures.
    #[cfg(test)]
    pub temporary_path: Option<PathBuf>,
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
        if self.thread.join().is_err() {
            eprintln!("Sentinel peripherals thread panicked");
        }
    }
}

/// Publish a `starting` catalog, then run the event loop on its own thread.
pub fn spawn(config: Config, providers: Vec<Box<dyn Provider>>) -> Result<PeripheralsHandle> {
    let control =
        Arc::new(Control::new(config.instance_id.clone()).context("create peripherals wake pipe")?);
    let mut catalog = Catalog::new(config.instance_id.clone(), DEFAULT_CHANGE_CAPACITY);
    let scanner = Scanner::new(
        providers,
        SupportStage::new(config.support_rules_path.clone()),
    );
    let uevents = match config
        .listen_for_uevents
        .then(|| UeventSocket::open(scanner.subsystems()))
    {
        None => None,
        Some(Ok(socket)) => Some(socket),
        Some(Err(error)) => {
            eprintln!("Sentinel cannot watch hot-plug events: {error}");
            catalog.apply_error(
                "peripherals.monitor_failed",
                &format!(
                    "hot-plug events are unavailable; only explicit refreshes rescan: {error}"
                ),
            );
            None
        }
    };
    // Without the directory (Core not installed yet), rules are still re-read
    // on every scan; Core's installer also requests a refresh.
    let rules_watch = RulesWatch::open(scanner.support_path())
        .map_err(|error| {
            eprintln!(
                "Sentinel is not watching {}: {error}",
                scanner.support_path().display()
            )
        })
        .ok();
    let publication = publish_config(&config, &catalog.document())?;
    {
        let mut schedule = control.schedule();
        schedule.publication = Some(publication);
        schedule.alive = true;
    }
    let thread_control = control.clone();
    let thread = thread::Builder::new()
        .name("peripherals".into())
        .spawn(move || {
            run(
                config,
                thread_control,
                catalog,
                scanner,
                uevents,
                rules_watch,
            )
        })
        .context("spawn peripherals thread")?;
    Ok(PeripheralsHandle { control, thread })
}

/// Marks the thread dead when it exits for any reason, including a panic.
struct AliveGuard(Arc<Control>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.schedule().alive = false;
    }
}

fn run(
    config: Config,
    control: Arc<Control>,
    mut catalog: Catalog,
    mut scanner: Scanner,
    mut uevents: Option<UeventSocket>,
    mut rules_watch: Option<RulesWatch>,
) {
    let _alive = AliveGuard(control.clone());
    lower_priority();
    // The initial scan runs immediately; afterwards the thread sleeps in
    // poll() until a uevent, a rules change, a refresh request, or shutdown.
    let mut due = Some(Instant::now());
    let mut reclassify_due: Option<Instant> = None;
    // A refresh is waiting on `due`: later events may not postpone it.
    let mut refresh_pending = false;
    // When the current burst of events began, to cap how long it can delay.
    let mut burst_start: Option<Instant> = None;
    // Refresh requests within the cooldown share the next scan, so a client
    // that spams the world-writable API cannot keep the thread busy.
    let mut last_scan_end: Option<Instant> = None;
    // While the catalog cannot be written, when to try again.
    let mut retry_publish: Option<Instant> = None;
    loop {
        // A pending scan re-reads the rules, so it supersedes a reclassify.
        let next = [due.or(reclassify_due), retry_publish]
            .into_iter()
            .flatten()
            .min();
        let timeout = next.map_or(-1, |at| {
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
            libc::pollfd {
                fd: rules_watch.as_ref().map_or(-1, RulesWatch::as_raw_fd),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is a valid array of three pollfd structures.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 3, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                eprintln!("Sentinel peripherals thread stopped: poll failed: {error}");
                catalog.apply_error(
                    "peripherals.monitor_failed",
                    &format!("waiting for events failed: {error}"),
                );
                let _ = publish_config(&config, &catalog.document());
                return;
            }
        }
        if fds[0].revents != 0 {
            control.drain_wake();
        }
        {
            let mut schedule = control.schedule();
            if schedule.stop {
                drop(schedule);
                // Like the metrics cache, leave no catalog that looks live.
                catalog.apply_error("peripherals.stopped", "The Sentinel daemon has stopped.");
                let _ = publish_config(&config, &catalog.document());
                return;
            }
            if std::mem::take(&mut schedule.refresh) {
                let earliest = refresh_due(Instant::now(), last_scan_end, config.refresh_cooldown);
                due = Some(due.map_or(earliest, |at| at.min(earliest)));
                refresh_pending = true;
            }
        }
        if fds[1].revents != 0 {
            if let Some(socket) = &uevents {
                match socket.drain() {
                    Ok(drained) if drained.matched || drained.overflowed => {
                        // Trailing debounce, capped so a flapping device cannot
                        // postpone the scan forever; never delays a refresh.
                        let now = Instant::now();
                        let start = *burst_start.get_or_insert(now);
                        due = Some(scan_due_after_event(
                            now,
                            start,
                            config.debounce,
                            due,
                            refresh_pending,
                        ));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("Sentinel stopped receiving hot-plug events: {error}");
                        catalog.apply_error(
                            "peripherals.monitor_failed",
                            &format!("hot-plug events stopped: {error}"),
                        );
                        uevents = None;
                        retry_publish = write(&control, &config, &catalog);
                    }
                }
            }
        }
        if fds[2].revents != 0 {
            if let Some(watch) = &rules_watch {
                match watch.drain() {
                    // A package install writes in steps; wait for it to settle.
                    Ok(true) => reclassify_due = Some(Instant::now() + config.debounce),
                    Ok(false) => {}
                    Err(error) => {
                        eprintln!("Sentinel stopped watching support rules: {error}");
                        rules_watch = None;
                    }
                }
            }
        }
        if due.is_none() && reclassify_due.is_some_and(|at| Instant::now() >= at) {
            reclassify_due = None;
            if let Err(error) = scanner.reclassify(&mut catalog) {
                eprintln!("Sentinel peripheral reclassification rejected: {error}");
            }
            retry_publish = write(&control, &config, &catalog);
        }
        if due.is_some_and(|at| Instant::now() >= at) {
            due = None;
            refresh_pending = false;
            burst_start = None;
            // A scan re-reads the rules too.
            reclassify_due = None;
            control.schedule().scanning = true;
            if let Err(error) = scanner.scan(&mut catalog) {
                eprintln!("Sentinel peripheral scan rejected: {error}");
            }
            {
                let mut schedule = control.schedule();
                schedule.scanning = false;
                schedule.completed = catalog.scan_sequence();
            }
            retry_publish = write(&control, &config, &catalog);
            last_scan_end = Some(Instant::now());
        }
        if retry_publish.is_some_and(|at| Instant::now() >= at) {
            retry_publish = write(&control, &config, &catalog);
        }
    }
}

fn refresh_due(now: Instant, last_scan_end: Option<Instant>, cooldown: Duration) -> Instant {
    last_scan_end.map_or(now, |end| (end + cooldown).max(now))
}

/// Publish the catalog and record the outcome in `control`, so the API
/// refuses the stale file while writes fail (for example, `/run` is full)
/// and serves it again once one succeeds. Returns when to retry, if it failed.
fn write(control: &Control, config: &Config, catalog: &Catalog) -> Option<Instant> {
    let (publication, error) = match publish_config(config, &catalog.document()) {
        Ok(publication) => (Some(publication), None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    let previous = {
        let mut schedule = control.schedule();
        if let Some(publication) = publication {
            schedule.publication = Some(publication);
        }
        std::mem::replace(&mut schedule.publish_error, error.clone())
    };
    match &error {
        // Retries repeat the same error every second; log each new one.
        Some(error) if previous.as_ref() != Some(error) => {
            eprintln!("Sentinel peripheral catalog write failed: {error}")
        }
        None if previous.is_some() => eprintln!("Sentinel peripheral catalog written again"),
        _ => {}
    }
    error.map(|_| Instant::now() + PUBLISH_RETRY)
}

/// When to scan after a hot-plug event: a trailing debounce, capped at
/// `MAX_EVENT_WAIT` after the burst began, and never later than a refresh
/// that is already waiting.
fn scan_due_after_event(
    now: Instant,
    burst_start: Instant,
    debounce: Duration,
    due: Option<Instant>,
    refresh_pending: bool,
) -> Instant {
    let event_due = (now + debounce).min(burst_start + MAX_EVENT_WAIT);
    match due {
        Some(at) if refresh_pending => at.min(event_due),
        _ => event_due,
    }
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

/// Replace the catalog file atomically so readers never see a partial write.
#[cfg(test)]
pub fn publish(path: &Path, document: &CatalogDocument) -> Result<()> {
    publish_identity(path, document).map(|_| ())
}

fn publish_identity(path: &Path, document: &CatalogDocument) -> Result<Publication> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("peripherals.json");
    let temporary = parent.join(format!(".{name}.{}.tmp", new_instance_id()));
    publish_to(path, document, &temporary)
}

fn publish_config(config: &Config, document: &CatalogDocument) -> Result<Publication> {
    #[cfg(test)]
    if let Some(temporary) = &config.temporary_path {
        return publish_to(&config.catalog_path, document, temporary);
    }
    publish_identity(&config.catalog_path, document)
}

fn publish_to(path: &Path, document: &CatalogDocument, temporary: &Path) -> Result<Publication> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let result = (|| {
        // The exclusive, randomized name cannot follow an attacker-created
        // symlink when the catalog is placed in a shared directory.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        serde_json::to_writer(&mut file, document).context("serialize peripheral catalog")?;
        file.write_all(b"\n")?;
        let publication = Publication::from_metadata(&file.metadata()?);
        fs::rename(temporary, path).with_context(|| format!("replace {}", path.display()))?;
        Ok(publication)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
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
    use crate::peripherals::scan::testing::{record, Fake};
    use crate::peripherals::support::core_rules;
    use crate::peripherals::sysutil::testing::TempDir;
    use serde_json::json;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    /// Start the thread on `<root>/peripherals.json`, with uevents off.
    fn start(root: &Path, debounce: u64, provider: impl Provider + 'static) -> PeripheralsHandle {
        let config = Config {
            catalog_path: root.join("peripherals.json"),
            support_rules_path: root.join("support/neat-core.json"),
            instance_id: "test-instance".into(),
            debounce: Duration::from_millis(debounce),
            refresh_cooldown: Duration::from_millis(debounce),
            listen_for_uevents: false,
            temporary_path: None,
        };
        spawn(config, vec![Box::new(provider)]).unwrap()
    }

    /// The first catalog under `root` to satisfy `predicate`, summarised as
    /// `state rev=… scan=… devices=… last=<newest change>`.
    fn wait_for(root: &Path, predicate: impl Fn(&CatalogDocument) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match read(&root.join("peripherals.json")) {
                Ok(d) if predicate(&d) => {
                    let (state, rev, scan) = (&d.state, d.revision, d.scan_sequence);
                    let (devices, last) = (d.devices.len(), d.changes.last());
                    let last = last.map_or("-", |change| &change.kind);
                    return format!("{state} rev={rev} scan={scan} devices={devices} last={last}");
                }
                _ => assert!(Instant::now() < deadline, "the catalog never got there"),
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The first scan is published at once; a refresh promises the scan that
    /// covers it, and a burst of refreshes shares one scan; nothing else
    /// scans. Scans run niced. Stopping marks the catalog stopped and refuses
    /// later refreshes.
    #[test]
    fn scans_on_start_and_refresh_until_stopped() {
        let root = TempDir::new();
        let (calls, nice) = (Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(None)));
        let (counter, seen) = (calls.clone(), nice.clone());
        let provider = Fake("test.camera", move || {
            // SAFETY: reads this thread's own scheduling priority.
            let tid = unsafe { libc::syscall(libc::SYS_gettid) } as libc::id_t;
            *seen.lock().unwrap() = Some(unsafe { libc::getpriority(libc::PRIO_PROCESS, tid) });
            let call = counter.fetch_add(1, SeqCst);
            let camera = |index| record("test.camera", &format!("camera:{index}"), json!({}));
            Ok((0..=call).map(camera).collect())
        });
        let handle = start(root.path(), 100, provider);
        let ready = wait_for(root.path(), |d| d.ready && d.instance_id == "test-instance");
        assert_eq!(ready, "ready rev=1 scan=1 devices=1 last=-");

        let control = handle.control();
        assert_eq!(control.request_refresh(), Some(2));
        let burst = (0..200).map(|_| control.request_refresh().unwrap());
        let target = burst.max().unwrap();
        assert!(target <= 3, "200 requests promise at most two more scans");
        let refreshed = wait_for(root.path(), |d| d.scan_sequence >= target);
        let expected = format!("ready rev={target} scan={target} devices={target} last=added");
        assert_eq!(refreshed, expected);

        handle.stop();
        let code = |d: &CatalogDocument| d.error.as_ref().map(|error| error["code"].clone());
        let stopped = |d: &CatalogDocument| code(d) == Some(json!("peripherals.stopped"));
        let (scans, revision) = (calls.load(SeqCst), calls.load(SeqCst) + 1);
        let expected = format!("degraded rev={revision} scan={scans} devices={scans} last=error");
        let stopped = wait_for(root.path(), stopped);
        assert_eq!(stopped, expected, "no scan without a trigger");
        assert_eq!(control.request_refresh(), None);
        // The process may already run niced; discovery adds NICE on top.
        let base = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        assert_eq!(*nice.lock().unwrap(), Some((base + NICE).min(19)));
    }

    #[test]
    fn installing_core_rules_reclassifies_without_rescanning() {
        let root = TempDir::new();
        let rules_dir = root.path().join("support");
        fs::create_dir_all(&rules_dir).unwrap();
        let mode = json!({"format": "NV12", "width": 1920, "height": 1080,
                          "framerate_num": 30, "framerate_den": 1, "isp_output": true});
        let details = json!({"backend": "mipi", "modes": [mode]});
        let camera = record("test.mipi", "camera:imx477 5-001a", details);
        let provider = Fake("test.mipi", move || Ok(vec![camera.clone()]));
        let handle = start(root.path(), 10, provider);
        let supported = |d: &CatalogDocument, source: Option<&str>| {
            let support = d.support.as_ref().unwrap().source.as_deref();
            d.devices[0]["camera"]["modes"][0]["supported"] == source.is_some() && support == source
        };
        let before = wait_for(root.path(), |d| d.ready && supported(d, None));
        assert_eq!(before, "ready rev=1 scan=1 devices=1 last=-");

        let staged = rules_dir.join("neat-core.json.dpkg-new");
        fs::write(&staged, core_rules().to_string()).unwrap();
        fs::rename(&staged, rules_dir.join("neat-core.json")).unwrap();
        let after = wait_for(root.path(), |d| supported(d, Some("neat-core 0.4.0")));
        assert_eq!(after, "ready rev=2 scan=1 devices=1 last=changed");
        handle.stop();
    }

    #[test]
    fn events_cannot_postpone_a_scan_forever_or_delay_a_refresh() {
        let (start, debounce) = (Instant::now(), Duration::from_millis(250));
        let due = |now, due, refresh| scan_due_after_event(now, start, debounce, due, refresh);
        let trailing = due(start, None, false);
        assert_eq!(trailing, start + debounce);
        let late = start + Duration::from_millis(900);
        let capped = due(late, Some(late), false);
        assert_eq!(capped, start + MAX_EVENT_WAIT);
        let refresh = due(start, Some(start), true);
        assert_eq!(refresh, start, "a waiting refresh is not delayed");
    }

    #[test]
    fn explicit_refreshes_respect_the_production_cooldown() {
        let end = Instant::now();
        let within = end + Duration::from_secs(1);
        assert_eq!(
            refresh_due(within, Some(end), REFRESH_COOLDOWN),
            end + REFRESH_COOLDOWN
        );
        let after = end + REFRESH_COOLDOWN + Duration::from_secs(1);
        assert_eq!(refresh_due(after, Some(end), REFRESH_COOLDOWN), after);
    }

    #[test]
    fn catalog_publication_uses_random_exclusive_temporary_files() {
        let root = TempDir::new();
        let path = root.path().join("peripherals.json");
        let target = root.path().join("target");
        fs::write(&target, "do not overwrite").unwrap();

        let attacker_path = root.path().join("attacker.tmp");
        symlink(&target, &attacker_path).unwrap();
        let document = Catalog::new("test-instance", DEFAULT_CHANGE_CAPACITY).document();
        assert!(publish_to(&path, &document, &attacker_path).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "do not overwrite");
        assert!(!path.exists());

        symlink(&target, root.path().join(".peripherals.json.tmp")).unwrap();
        publish(&path, &document).unwrap();

        assert_eq!(fs::read_to_string(target).unwrap(), "do not overwrite");
        assert_eq!(read(&path).unwrap().instance_id, "test-instance");
    }
}
