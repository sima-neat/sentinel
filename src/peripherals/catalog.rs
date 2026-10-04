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
    support_changed: bool,
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
            support_changed: false,
        }
    }

    pub fn set_support(&mut self, support: SupportStatus) {
        if self.support.as_ref() != Some(&support) {
            self.support_changed = true;
        }
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
        canonicalize_devices(&mut devices)?;
        canonicalize_issues(&mut issues)?;
        let recovered = !self.issues.is_empty() && issues.is_empty();
        let issues_changed = self.issues != issues;
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
            self.support_changed = false;
        } else if devices_changed || issues_changed || std::mem::take(&mut self.support_changed) {
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
    pub fn apply_rejected_scan(&mut self, reason: &str) {
        self.apply_failed_scan(vec![Issue {
            provider: "catalog".into(),
            code: "peripherals.invalid_provider_result".into(),
            reason: reason.into(),
            retained_last_good: self.initialized,
        }]);
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

    fn camera(id: &str, model: &str) -> Record {
        Record {
            id: id.into(),
            kind: "camera".into(),
            provider: "test.camera".into(),
            details: json!({"model": model}),
        }
    }

    fn issue(retained: bool) -> Issue {
        Issue {
            provider: "test.camera".into(),
            code: "io.permission_denied".into(),
            reason: "permission denied".into(),
            retained_last_good: retained,
        }
    }

    #[test]
    fn starts_unready_and_first_scan_is_revision_one() {
        let mut catalog = Catalog::new("a", 8);
        let starting = catalog.document();
        assert_eq!(starting.state, "starting");
        assert!(!starting.ready);
        assert_eq!(starting.revision, 0);

        catalog.apply_success(vec![], vec![]).unwrap();
        let ready = catalog.document();
        assert_eq!(ready.state, "ready");
        assert!(ready.ready);
        assert_eq!((ready.revision, ready.scan_sequence), (1, 1));
        assert!(ready.devices.is_empty());
        assert!(ready.last_success_at.is_some());
    }

    #[test]
    fn unchanged_scan_advances_scan_sequence_but_not_revision() {
        let mut catalog = Catalog::new("a", 8);
        catalog
            .apply_success(vec![camera("c1", "x")], vec![])
            .unwrap();
        catalog
            .apply_success(vec![camera("c1", "x")], vec![])
            .unwrap();
        let document = catalog.document();
        assert_eq!((document.revision, document.scan_sequence), (1, 2));
        assert!(document.changes.is_empty());
    }

    #[test]
    fn diff_emits_added_removed_and_changed_in_one_revision() {
        let mut catalog = Catalog::new("a", 8);
        catalog
            .apply_success(vec![camera("a", "1"), camera("b", "1")], vec![])
            .unwrap();
        catalog
            .apply_success(vec![camera("b", "2"), camera("c", "1")], vec![])
            .unwrap();
        let document = catalog.document();
        assert_eq!(document.revision, 2);
        let kinds: Vec<_> = document
            .changes
            .iter()
            .map(|change| (change.kind.as_str(), change.device_id.as_deref().unwrap()))
            .collect();
        assert_eq!(
            kinds,
            vec![("removed", "a"), ("changed", "b"), ("added", "c")]
        );
        assert!(document.changes.iter().all(|change| change.revision == 2));
        assert_eq!(document.sequence, 3);
        assert_eq!(document.devices[0]["camera"]["model"], "2");
    }

    #[test]
    fn retained_issue_marks_catalog_stale_and_recovery_is_logged() {
        let mut catalog = Catalog::new("a", 8);
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        catalog
            .apply_success(vec![camera("a", "1")], vec![issue(true)])
            .unwrap();
        let degraded = catalog.document();
        assert_eq!(degraded.state, "degraded");
        assert!(degraded.stale);
        assert!(degraded.error.is_none());
        let logged = degraded.changes.last().unwrap();
        assert_eq!(logged.kind, "error");
        assert_eq!(
            logged.error.as_ref().unwrap()["code"],
            "peripherals.provider_degraded"
        );

        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        let recovered = catalog.document();
        assert_eq!(recovered.state, "ready");
        assert!(!recovered.stale);
        assert_eq!(recovered.changes.last().unwrap().kind, "recovered");
        assert_eq!(
            (degraded.revision, recovered.revision),
            (2, 3),
            "issue changes are visible changes"
        );
    }

    #[test]
    fn provider_failure_before_any_success_stays_unready() {
        let mut catalog = Catalog::new("a", 8);
        catalog.apply_provider_failure(vec![issue(false)]).unwrap();
        let document = catalog.document();
        assert!(!document.ready);
        assert_eq!(document.state, "degraded");
        assert!(!document.stale);
        assert_eq!(document.scan_sequence, 1);
        assert!(catalog.apply_provider_failure(vec![]).is_err());
    }

    #[test]
    fn duplicate_identities_and_empty_issue_fields_are_rejected() {
        let mut catalog = Catalog::new("a", 8);
        assert!(catalog
            .apply_success(vec![camera("a", "1"), camera("a", "2")], vec![])
            .is_err());
        let mut bad = issue(false);
        bad.reason.clear();
        assert!(catalog.apply_success(vec![], vec![bad]).is_err());
        assert_eq!(catalog.scan_sequence(), 0);
    }

    #[test]
    fn reclassification_changes_revision_but_not_scan_sequence() {
        let mut catalog = Catalog::new("a", 8);
        catalog
            .apply_reclassification(vec![camera("a", "1")], vec![])
            .unwrap();
        assert_eq!(
            catalog.document().revision,
            0,
            "nothing before the first scan"
        );
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        let attempted = catalog.document().last_attempt_at;
        catalog
            .apply_reclassification(vec![camera("a", "2")], vec![])
            .unwrap();
        let document = catalog.document();
        assert_eq!((document.revision, document.scan_sequence), (2, 1));
        assert_eq!(document.last_attempt_at, attempted);
        assert_eq!(document.changes.last().unwrap().kind, "changed");
    }

    #[test]
    fn monitor_error_survives_later_scans_and_is_never_reported_recovered() {
        let mut catalog = Catalog::new("a", 8);
        catalog.apply_error("peripherals.monitor_failed", "uevent socket unavailable");
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        let document = catalog.document();
        assert_eq!(document.state, "degraded");
        assert!(document.stale);
        assert_eq!(
            document.error.unwrap()["code"],
            "peripherals.monitor_failed"
        );
        assert!(document
            .changes
            .iter()
            .all(|change| change.kind != "recovered"));
    }

    #[test]
    fn unchanged_rescans_and_repeated_errors_keep_the_revision() {
        let mut catalog = Catalog::new("a", 8);
        catalog
            .apply_success(vec![camera("a", "1")], vec![issue(true)])
            .unwrap();
        catalog
            .apply_success(vec![camera("a", "1")], vec![issue(true)])
            .unwrap();
        catalog.apply_error("peripherals.monitor_failed", "x");
        let first = catalog.document();
        catalog.apply_error("peripherals.monitor_failed", "x");
        let second = catalog.document();
        assert_eq!(first.revision, 2);
        assert_eq!(
            (second.revision, second.sequence),
            (first.revision, first.sequence)
        );
    }

    #[test]
    fn change_log_is_bounded() {
        let mut catalog = Catalog::new("a", 2);
        for index in 0..5 {
            catalog
                .apply_success(vec![camera(&format!("c{index}"), "x")], vec![])
                .unwrap();
        }
        let document = catalog.document();
        assert_eq!(document.changes.len(), 2);
        assert_eq!(document.changes[1].sequence, document.sequence);
    }

    #[test]
    fn monitor_error_marks_initialized_catalog_stale() {
        let mut catalog = Catalog::new("a", 8);
        catalog
            .apply_success(vec![camera("a", "1")], vec![])
            .unwrap();
        catalog.apply_error("peripherals.monitor_failed", "uevent socket closed");
        let document = catalog.document();
        assert!(document.stale);
        assert_eq!(
            document.error.unwrap()["code"],
            "peripherals.monitor_failed"
        );
        assert_eq!(document.scan_sequence, 1);
    }
}
