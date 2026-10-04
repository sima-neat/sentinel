use std::collections::VecDeque;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use super::model::{CatalogDocument, Change, Issue, Record, SCHEMA_VERSION};
use super::support::SupportStatus;

pub const DEFAULT_CHANGE_CAPACITY: usize = 256;

/// Authoritative catalog state. Only the peripherals thread mutates it; every
/// reader sees the published [`CatalogDocument`].
pub struct Catalog {
    instance_id: String,
    change_capacity: usize,
    devices: Vec<Record>,
    issues: Vec<Issue>,
    changes: VecDeque<Change>,
    revision: u64,
    sequence: u64,
    scan_sequence: u64,
    initialized: bool,
    last_success_at: Option<String>,
    last_attempt_at: Option<String>,
    error: Option<Value>,
    support: Option<SupportStatus>,
}

impl Catalog {
    pub fn new(instance_id: impl Into<String>, change_capacity: usize) -> Self {
        Self {
            instance_id: instance_id.into(),
            change_capacity: change_capacity.max(1),
            devices: Vec::new(),
            issues: Vec::new(),
            changes: VecDeque::new(),
            revision: 0,
            sequence: 0,
            scan_sequence: 0,
            initialized: false,
            last_success_at: None,
            last_attempt_at: None,
            error: None,
            support: None,
        }
    }

    pub fn scan_sequence(&self) -> u64 {
        self.scan_sequence
    }

    /// Record a scan in which at least one provider has usable records.
    #[cfg(test)]
    pub fn apply_success(
        &mut self,
        devices: Vec<Record>,
        issues: Vec<Issue>,
    ) -> Result<(), String> {
        self.apply(devices, issues, true, None)
    }

    pub fn apply_success_with_support(
        &mut self,
        devices: Vec<Record>,
        issues: Vec<Issue>,
        support: SupportStatus,
    ) -> Result<(), String> {
        self.apply(devices, issues, true, Some(support))
    }

    /// Republish the last scan's records after the support rules changed.
    /// No hardware was read, so `scan_sequence` and the timestamps stay put.
    #[cfg(test)]
    pub fn apply_reclassification(
        &mut self,
        devices: Vec<Record>,
        issues: Vec<Issue>,
    ) -> Result<(), String> {
        if !self.initialized {
            return Ok(());
        }
        self.apply(devices, issues, false, None)
    }

    pub fn apply_reclassification_with_support(
        &mut self,
        devices: Vec<Record>,
        issues: Vec<Issue>,
        support: SupportStatus,
    ) -> Result<(), String> {
        if !self.initialized {
            return Ok(());
        }
        self.apply(devices, issues, false, Some(support))
    }

    fn apply(
        &mut self,
        mut devices: Vec<Record>,
        mut issues: Vec<Issue>,
        scanned: bool,
        support: Option<SupportStatus>,
    ) -> Result<(), String> {
        canonicalize_devices(&mut devices)?;
        canonicalize_issues(&mut issues)?;
        let recovered = !self.issues.is_empty() && issues.is_empty();
        let issues_changed = self.issues != issues;
        let support_changed = support
            .as_ref()
            .is_some_and(|status| self.support.as_ref() != Some(status));
        if scanned {
            let now = utc_now();
            self.scan_sequence += 1;
            self.last_attempt_at = Some(now.clone());
            if issues.is_empty() {
                self.last_success_at = Some(now);
            }
        }

        let first = !self.initialized;
        let devices_changed = self.devices != devices;
        if first {
            self.initialized = true;
            self.revision = 1;
        } else if devices_changed || issues_changed || support_changed {
            // `revision` changes whenever anything a client can see changes,
            // so polling with `since_revision` never hides a new state.
            self.revision += 1;
        }
        if recovered {
            self.push_change("recovered", None, None, None);
        }
        if issues_changed && !issues.is_empty() {
            self.push_change("error", None, None, Some(issue_error(&issues)));
        }
        // The first scan logs no device changes.
        if !first && devices_changed {
            let previous = std::mem::take(&mut self.devices);
            let (mut old, mut new) = (0, 0);
            while old < previous.len() || new < devices.len() {
                if new == devices.len()
                    || (old < previous.len() && previous[old].id < devices[new].id)
                {
                    self.push_device_change("removed", &previous[old]);
                    old += 1;
                } else if old == previous.len() || devices[new].id < previous[old].id {
                    self.push_device_change("added", &devices[new]);
                    new += 1;
                } else {
                    if previous[old] != devices[new] {
                        self.push_device_change("changed", &devices[new]);
                    }
                    old += 1;
                    new += 1;
                }
            }
        }
        self.devices = devices;
        self.issues = issues;
        if let Some(support) = support {
            self.support = Some(support);
        }
        Ok(())
    }

    /// Record a scan in which no provider has ever produced records.
    pub fn apply_provider_failure(&mut self, mut issues: Vec<Issue>) -> Result<(), String> {
        canonicalize_issues(&mut issues)?;
        if issues.is_empty() {
            return Err("a provider failure scan must contain an issue".into());
        }
        self.apply_failed_scan(issues);
        Ok(())
    }

    /// Record a scan whose combined result was invalid (a provider bug such as
    /// two providers returning one id). Devices keep their previous values;
    /// the scan still counts, so refresh targets are reached.
    pub fn apply_rejected_scan(&mut self, reason: &str) -> Issue {
        let issue = Issue {
            provider: "catalog".into(),
            code: "peripherals.invalid_provider_result".into(),
            reason: reason.into(),
            retained_last_good: self.initialized,
        };
        self.apply_failed_scan(vec![issue.clone()]);
        issue
    }

    /// Count a scan that left the devices as they were; a new set of issues
    /// is a visible change.
    fn apply_failed_scan(&mut self, issues: Vec<Issue>) {
        self.scan_sequence += 1;
        self.last_attempt_at = Some(utc_now());
        if self.issues != issues {
            if self.initialized {
                self.revision += 1;
            }
            self.push_change("error", None, None, Some(issue_error(&issues)));
            self.issues = issues;
        }
    }

    /// Record a failure of the event monitor or the thread itself. Unlike a
    /// provider issue it is not cleared by later scans: it describes the
    /// daemon, and stays until the daemon restarts.
    pub fn apply_error(&mut self, code: &str, reason: &str) {
        let error = json!({"code": code, "reason": reason});
        if self.error.as_ref() == Some(&error) {
            return;
        }
        if self.initialized {
            self.revision += 1;
        }
        self.error = Some(error.clone());
        self.push_change("error", None, None, Some(error));
    }

    pub fn document(&self) -> CatalogDocument {
        let degraded = self.error.is_some() || !self.issues.is_empty();
        let retained = self.issues.iter().any(|issue| issue.retained_last_good);
        let state = if degraded {
            "degraded"
        } else if self.initialized {
            "ready"
        } else {
            "starting"
        };
        CatalogDocument {
            schema_version: SCHEMA_VERSION,
            instance_id: self.instance_id.clone(),
            state: state.into(),
            ready: self.initialized,
            stale: self.initialized && (self.error.is_some() || retained),
            revision: self.revision,
            sequence: self.sequence,
            scan_sequence: self.scan_sequence,
            last_success_at: self.last_success_at.clone(),
            last_attempt_at: self.last_attempt_at.clone(),
            error: self.error.clone(),
            issues: self.issues.clone(),
            changes: self.changes.iter().cloned().collect(),
            support: self.support.clone(),
            devices: self.devices.iter().map(Record::to_catalog_value).collect(),
        }
    }

    fn push_device_change(&mut self, kind: &str, record: &Record) {
        self.push_change(
            kind,
            Some(record.id.clone()),
            Some(record.kind.clone()),
            None,
        );
    }

    fn push_change(
        &mut self,
        kind: &str,
        device_id: Option<String>,
        device_type: Option<String>,
        error: Option<Value>,
    ) {
        self.sequence += 1;
        if self.changes.len() == self.change_capacity {
            self.changes.pop_front();
        }
        self.changes.push_back(Change {
            sequence: self.sequence,
            revision: self.revision,
            kind: kind.into(),
            device_id,
            device_type,
            error,
        });
    }
}

pub fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn canonicalize_devices(devices: &mut [Record]) -> Result<(), String> {
    devices.sort_by(|left, right| left.id.cmp(&right.id));
    if let Some(pair) = devices.windows(2).find(|pair| pair[0].id == pair[1].id) {
        return Err(format!("duplicate peripheral identity: {}", pair[0].id));
    }
    Ok(())
}

fn canonicalize_issues(issues: &mut [Issue]) -> Result<(), String> {
    issues.sort();
    if issues
        .iter()
        .any(|issue| issue.provider.is_empty() || issue.code.is_empty() || issue.reason.is_empty())
    {
        return Err("peripheral provider issue fields must not be empty".into());
    }
    if let Some(pair) = issues
        .windows(2)
        .find(|pair| pair[0].provider == pair[1].provider)
    {
        return Err(format!(
            "duplicate peripheral provider issue: {}",
            pair[0].provider
        ));
    }
    Ok(())
}

fn issue_error(issues: &[Issue]) -> Value {
    json!({
        "code": "peripherals.provider_degraded",
        "reason": "One or more peripheral providers could not be refreshed.",
        "issues": issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cameras from `"id:model"` pairs.
    fn cameras(pairs: &[&str]) -> Vec<Record> {
        let camera = |pair: &&str| {
            let (id, model) = pair.split_once(':').unwrap();
            crate::peripherals::scan::testing::record("p", id, json!({"model": model}))
        };
        pairs.iter().map(camera).collect()
    }

    fn issue(retained: bool) -> Issue {
        let issue = json!({"provider": "p", "code": "io.permission_denied", "reason": "denied",
                           "retained_last_good": retained});
        serde_json::from_value(issue).unwrap()
    }

    /// `state ready=… stale=… rev=… scan=… seq=…` of the document.
    #[track_caller]
    fn state(catalog: &Catalog, expected: &str) {
        let d = catalog.document();
        let (state, ready, stale) = (d.state, d.ready, d.stale);
        let (rev, scan, seq) = (d.revision, d.scan_sequence, d.sequence);
        let text = format!("{state} ready={ready} stale={stale} rev={rev} scan={scan} seq={seq}");
        assert_eq!(text, expected);
    }

    /// The change log as `kind subject@revision`, the subject being a device
    /// id or an error code.
    #[track_caller]
    fn log(catalog: &Catalog, expected: &str) {
        let entry = |change: &Change| {
            let error = change.error.as_ref().map(|e| e["code"].as_str().unwrap());
            let subject = change.device_id.as_deref().or(error).unwrap_or("-");
            format!("{} {subject}@{}", change.kind, change.revision)
        };
        let changes = catalog.document().changes;
        let changes = Vec::from_iter(changes.iter().map(entry));
        assert_eq!(changes.join(", "), expected);
    }

    /// The first scan is revision 1 and logs no device changes; an unchanged
    /// scan advances only `scan_sequence`; one revision carries every device
    /// change. Reclassification changes the revision but not `scan_sequence`
    /// or the timestamps, and does nothing before the first scan.
    #[test]
    fn revisions_follow_visible_changes() {
        let mut c = Catalog::new("a", 8);
        let starting = "starting ready=false stale=false rev=0 scan=0 seq=0";
        state(&c, starting);
        c.apply_reclassification(cameras(&["a:1"]), vec![]).unwrap();
        state(&c, starting);
        c.apply_success(cameras(&["b:1", "a:1"]), vec![]).unwrap();
        c.apply_success(cameras(&["a:1", "b:1"]), vec![]).unwrap();
        state(&c, "ready ready=true stale=false rev=1 scan=2 seq=0");
        log(&c, "");
        assert!(c.document().last_success_at.is_some());
        c.apply_success(cameras(&["b:2", "c:1"]), vec![]).unwrap();
        assert_eq!(c.document().devices[0]["camera"]["model"], "2");
        let attempted = c.document().last_attempt_at;
        let devices = cameras(&["b:3", "c:1"]);
        c.apply_reclassification(devices, vec![]).unwrap();
        state(&c, "ready ready=true stale=false rev=3 scan=3 seq=4");
        log(&c, "removed a@2, changed b@2, added c@2, changed b@3");
        assert_eq!(c.document().last_attempt_at, attempted);
    }

    /// Provider issues degrade the catalog, are logged and are visible
    /// changes; issues on retained records mark it stale. Invalid input is
    /// rejected without counting a scan. A monitor error survives later
    /// scans and is never reported recovered; repeating it changes nothing.
    #[test]
    fn issues_and_errors_degrade_the_catalog() {
        let mut c = Catalog::new("a", 8);
        c.apply_provider_failure(vec![issue(false)]).unwrap();
        assert!(c.apply_provider_failure(vec![]).is_err());
        assert!(c.apply_success(cameras(&["a:1", "a:2"]), vec![]).is_err());
        let mut empty_reason = issue(false);
        empty_reason.reason.clear();
        assert!(c.apply_success(vec![], vec![empty_reason]).is_err());
        state(&c, "degraded ready=false stale=false rev=0 scan=1 seq=1");
        let degraded = "error peripherals.provider_degraded";
        log(&c, &format!("{degraded}@0"));
        assert!(c.document().last_success_at.is_none());

        let a1 = || cameras(&["a:1"]);
        c.apply_success(a1(), vec![]).unwrap();
        c.apply_success(a1(), vec![issue(true)]).unwrap();
        c.apply_success(a1(), vec![issue(true)]).unwrap();
        state(&c, "degraded ready=true stale=true rev=2 scan=4 seq=3");
        c.apply_success(a1(), vec![]).unwrap();
        state(&c, "ready ready=true stale=false rev=3 scan=5 seq=4");
        let recovered = format!("{degraded}@0, recovered -@1, {degraded}@2, recovered -@3");
        log(&c, &recovered);

        c.apply_error("peripherals.monitor_failed", "uevent socket closed");
        c.apply_error("peripherals.monitor_failed", "uevent socket closed");
        c.apply_success(a1(), vec![]).unwrap();
        state(&c, "degraded ready=true stale=true rev=4 scan=6 seq=5");
        let monitor = "error peripherals.monitor_failed";
        log(&c, &format!("{recovered}, {monitor}@4"));
    }

    #[test]
    fn change_log_is_bounded() {
        let mut c = Catalog::new("a", 2);
        c.apply_error("peripherals.monitor_failed", "x");
        state(&c, "degraded ready=false stale=false rev=0 scan=0 seq=1");
        for index in 0..3 {
            let devices = cameras(&[&format!("c{index}:x")]);
            c.apply_success(devices, vec![]).unwrap();
        }
        state(&c, "degraded ready=true stale=true rev=3 scan=3 seq=5");
        log(&c, "removed c1@3, added c2@3");
    }
}
