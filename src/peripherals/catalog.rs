use std::collections::VecDeque;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use super::model::{CatalogDocument, Change, Issue, Record, SCHEMA_VERSION};
use super::support::SupportStatus;

/// How many recent changes the catalog publishes.
pub const CHANGE_LOG_CAPACITY: usize = 256;

/// Authoritative catalog state. Only the peripherals thread mutates it; every
/// reader sees the published [`CatalogDocument`].
pub struct Catalog {
    instance_id: String,
    devices: Vec<Record>,
    issues: Vec<Issue>,
    changes: VecDeque<Change>,
    revision: u64,
    sequence: u64,
    scan_sequence: u64,
    initialized: bool,
    last_success_at: Option<String>,
    last_attempt_at: Option<String>,
    /// A failure of the daemon itself (event monitor, shutdown). Scans never
    /// clear it; it lasts until the daemon restarts.
    error: Option<Value>,
    support: Option<SupportStatus>,
    support_changed: bool,
}

impl Catalog {
    pub fn new(instance_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
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
            support_changed: false,
        }
    }

    pub fn set_support(&mut self, support: SupportStatus) {
        self.support_changed |= self.support.as_ref() != Some(&support);
        self.support = Some(support);
    }

    pub fn scan_sequence(&self) -> u64 {
        self.scan_sequence
    }

    /// Record a scan in which at least one provider has usable records.
    pub fn apply_success(
        &mut self,
        devices: Vec<Record>,
        issues: Vec<Issue>,
    ) -> Result<(), String> {
        self.apply(devices, issues, true)
    }

    /// Republish the last scan's records after the support rules changed.
    /// No hardware was read, so `scan_sequence` and the timestamps stay put.
    pub fn apply_reclassification(
        &mut self,
        devices: Vec<Record>,
        issues: Vec<Issue>,
    ) -> Result<(), String> {
        if !self.initialized {
            return Ok(());
        }
        self.apply(devices, issues, false)
    }

    fn apply(
        &mut self,
        mut devices: Vec<Record>,
        mut issues: Vec<Issue>,
        scanned: bool,
    ) -> Result<(), String> {
        devices.sort_by(|left, right| left.id.cmp(&right.id));
        if let Some(pair) = devices.windows(2).find(|pair| pair[0].id == pair[1].id) {
            return Err(format!("duplicate peripheral identity: {}", pair[0].id));
        }
        issues.sort();
        if scanned {
            self.record_attempt(issues.is_empty());
        }
        if !self.initialized {
            self.initialized = true;
            self.revision = 1;
            self.support_changed = false;
            self.devices = devices;
            self.set_issues(issues);
            return Ok(());
        }
        let devices_changed = self.devices != devices;
        // `revision` changes whenever anything a client can see changes, so
        // polling with `since_revision` never hides a new state.
        if devices_changed || self.issues != issues || std::mem::take(&mut self.support_changed) {
            self.revision += 1;
        }
        self.set_issues(issues);
        if devices_changed {
            let previous = std::mem::replace(&mut self.devices, devices);
            self.log_device_changes(&previous);
        }
        Ok(())
    }

    /// Record a scan that produced no usable records: every provider failed
    /// before ever succeeding, or the combined result was invalid. Devices
    /// keep their previous values; the scan still counts, so refresh targets
    /// are reached.
    pub fn apply_issues_only(&mut self, mut issues: Vec<Issue>) {
        issues.sort();
        self.record_attempt(false);
        if self.initialized && self.issues != issues {
            self.revision += 1;
        }
        self.set_issues(issues);
    }

    /// Record a failure of the event monitor or the daemon itself.
    pub fn apply_error(&mut self, code: &str, reason: &str) {
        let error = json!({"code": code, "reason": reason});
        if self.error.as_ref() == Some(&error) {
            return;
        }
        if self.initialized {
            self.revision += 1;
        }
        self.error = Some(error.clone());
        self.push_change("error", None, Some(error));
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

    fn record_attempt(&mut self, succeeded: bool) {
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        self.scan_sequence += 1;
        if succeeded {
            self.last_success_at = Some(now.clone());
        }
        self.last_attempt_at = Some(now);
    }

    /// Replace the issues, logging `error` for a new set and `recovered` when
    /// the last one clears.
    fn set_issues(&mut self, issues: Vec<Issue>) {
        if issues != self.issues {
            if issues.is_empty() {
                self.push_change("recovered", None, None);
            } else {
                let error = json!({
                    "code": "peripherals.provider_degraded",
                    "reason": "One or more peripheral providers could not be refreshed.",
                    "issues": issues,
                });
                self.push_change("error", None, Some(error));
            }
        }
        self.issues = issues;
    }

    /// Log added, removed and changed devices against `previous`; both lists
    /// are sorted by id.
    fn log_device_changes(&mut self, previous: &[Record]) {
        let current = std::mem::take(&mut self.devices);
        let (mut old, mut new) = (previous.iter().peekable(), current.iter().peekable());
        loop {
            let change = match (old.peek(), new.peek()) {
                (None, None) => break,
                (Some(gone), next) if next.is_none_or(|next| gone.id < next.id) => {
                    ("removed", old.next())
                }
                (gone, Some(added)) if gone.is_none_or(|gone| added.id < gone.id) => {
                    ("added", new.next())
                }
                _ => {
                    let before = old.next();
                    let after = new.next();
                    if before == after {
                        continue;
                    }
                    ("changed", after)
                }
            };
            if let (kind, Some(record)) = change {
                self.push_change(kind, Some(record), None);
            }
        }
        self.devices = current;
    }

    fn push_change(&mut self, kind: &str, record: Option<&Record>, error: Option<Value>) {
        self.sequence += 1;
        if self.changes.len() == CHANGE_LOG_CAPACITY {
            self.changes.pop_front();
        }
        self.changes.push_back(Change {
            sequence: self.sequence,
            revision: self.revision,
            kind: kind.into(),
            device_id: record.map(|record| record.id.clone()),
            device_type: record.map(|record| record.kind.clone()),
            error,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(id: &str, model: &str) -> Record {
        Record {
            id: id.into(),
            kind: "camera".into(),
            provider: "test.camera".into(),
            details: json!({"model": model}),
        }
    }

    fn issue() -> Issue {
        Issue {
            provider: "test.camera".into(),
            code: "io.permission_denied".into(),
            reason: "permission denied".into(),
            retained_last_good: true,
        }
    }

    fn kinds(catalog: &Catalog) -> Vec<String> {
        catalog
            .document()
            .changes
            .iter()
            .map(|change| change.kind.clone())
            .collect()
    }

    #[test]
    fn first_scan_is_revision_one_and_unchanged_scans_keep_it() {
        let mut catalog = Catalog::new("a");
        assert_eq!(catalog.document().state, "starting");
        catalog
            .apply_success(vec![camera("c1", "x")], vec![])
            .unwrap();
        catalog
            .apply_success(vec![camera("c1", "x")], vec![])
            .unwrap();
        let document = catalog.document();
        assert_eq!(
            (
                document.state.as_str(),
                document.revision,
                document.scan_sequence
            ),
            ("ready", 1, 2)
        );
        assert!(document.changes.is_empty());
    }

    #[test]
    fn device_diff_is_logged_in_one_revision() {
        let mut catalog = Catalog::new("a");
        catalog
            .apply_success(vec![camera("a", "1"), camera("b", "1")], vec![])
            .unwrap();
        catalog
            .apply_success(vec![camera("b", "2"), camera("c", "1")], vec![])
            .unwrap();
        let document = catalog.document();
        let logged: Vec<_> = document
            .changes
            .iter()
            .map(|change| {
                (
                    change.kind.as_str(),
                    change.device_id.as_deref().unwrap(),
                    change.revision,
                )
            })
            .collect();
        assert_eq!(
            logged,
            [("removed", "a", 2), ("changed", "b", 2), ("added", "c", 2)]
        );
    }

    #[test]
    fn issues_mark_the_catalog_stale_bump_revision_and_recover() {
        let mut catalog = Catalog::new("a");
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        catalog
            .apply_success(vec![camera("a", "1")], vec![issue()])
            .unwrap();
        let degraded = catalog.document();
        assert_eq!(
            (degraded.state.as_str(), degraded.stale, degraded.revision),
            ("degraded", true, 2)
        );
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        let recovered = catalog.document();
        assert_eq!(
            (
                recovered.state.as_str(),
                recovered.stale,
                recovered.revision
            ),
            ("ready", false, 3)
        );
        assert_eq!(kinds(&catalog), ["error", "recovered"]);
    }

    #[test]
    fn issue_only_scans_count_and_keep_devices() {
        let mut catalog = Catalog::new("a");
        catalog.apply_issues_only(vec![issue()]);
        assert!(!catalog.document().ready, "no records yet");
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        catalog.apply_issues_only(vec![issue()]);
        let document = catalog.document();
        assert_eq!((document.scan_sequence, document.devices.len()), (3, 1));
        assert!(catalog
            .apply_success(vec![camera("a", "1"), camera("a", "2")], vec![])
            .is_err());
    }

    #[test]
    fn reclassification_changes_revision_but_not_scan_sequence() {
        let mut catalog = Catalog::new("a");
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        catalog
            .apply_reclassification(vec![camera("a", "2")], vec![])
            .unwrap();
        let document = catalog.document();
        assert_eq!((document.revision, document.scan_sequence), (2, 1));
        assert_eq!(kinds(&catalog), ["changed"]);
    }

    #[test]
    fn daemon_errors_persist_across_scans_and_are_logged_once() {
        let mut catalog = Catalog::new("a");
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        catalog.apply_error("peripherals.monitor_failed", "uevent socket closed");
        catalog.apply_error("peripherals.monitor_failed", "uevent socket closed");
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        let document = catalog.document();
        assert!(document.stale);
        assert_eq!(
            document.error.unwrap()["code"],
            "peripherals.monitor_failed"
        );
        assert_eq!((document.revision, kinds(&catalog).len()), (2, 1));
    }

    #[test]
    fn change_log_is_bounded() {
        let mut catalog = Catalog::new("a");
        for index in 0..=CHANGE_LOG_CAPACITY {
            catalog
                .apply_success(vec![camera(&format!("c{index}"), "x")], vec![])
                .unwrap();
        }
        let document = catalog.document();
        assert_eq!(document.changes.len(), CHANGE_LOG_CAPACITY);
        assert_eq!(document.changes.last().unwrap().sequence, document.sequence);
    }
}
