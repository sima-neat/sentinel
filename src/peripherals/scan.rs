use std::collections::BTreeSet;
use std::thread;

use super::catalog::Catalog;
use super::model::{Issue, Provider, ProviderError, Record};
use super::support::SupportStage;
use super::sysutil::CODE_DISCOVERY_FAILED;

/// A provider plus the records it returned in its last successful scan.
pub struct ProviderSlot {
    provider: Box<dyn Provider>,
    last_good: Option<Vec<Record>>,
}

impl ProviderSlot {
    pub fn new(provider: Box<dyn Provider>) -> Self {
        Self {
            provider,
            last_good: None,
        }
    }

    pub fn name(&self) -> &str {
        self.provider.name()
    }
}

/// Owns the providers, their last-good records, and the support stage.
pub struct Scanner {
    slots: Vec<ProviderSlot>,
    provider_issues: Vec<Issue>,
    support: SupportStage,
}

impl Scanner {
    pub fn new(providers: Vec<Box<dyn Provider>>, support: SupportStage) -> Self {
        Self {
            slots: providers.into_iter().map(ProviderSlot::new).collect(),
            provider_issues: Vec::new(),
            support,
        }
    }

    pub fn subsystems(&self) -> Vec<String> {
        self.slots
            .iter()
            .flat_map(|slot| slot.provider.subsystems().iter().cloned())
            .collect()
    }

    pub fn support_path(&self) -> &std::path::Path {
        self.support.path()
    }

    /// Run every provider, apply the support rules, then publish to the
    /// catalog, which compares against the previous result.
    pub fn scan(&mut self, catalog: &mut Catalog) -> Result<(), String> {
        let issues = discover_all(&mut self.slots);
        self.provider_issues = issues.clone();
        let (devices, has_provider_result) = compose(&self.slots);
        // No providers at all is a valid, empty catalog rather than a failure.
        if has_provider_result || issues.is_empty() {
            let (devices, issues, support) = self.classify(devices, issues);
            // Providers own distinct id prefixes; a collision is a provider
            // bug. It still counts as a scan so refresh targets are reached.
            catalog
                .apply_success_with_support(devices, issues, support)
                .inspect_err(|reason| {
                    catalog.apply_rejected_scan(reason);
                })
        } else {
            catalog.apply_provider_failure(issues)
        }
    }

    /// Re-apply changed support rules to the last scan's records without
    /// reading any hardware.
    pub fn reclassify(&mut self, catalog: &mut Catalog) -> Result<(), String> {
        let (devices, has_provider_result) = compose(&self.slots);
        if !has_provider_result {
            return Ok(());
        }
        let (devices, issues, support) = self.classify(devices, self.provider_issues.clone());
        catalog.apply_reclassification_with_support(devices, issues, support)
    }

    fn classify(
        &mut self,
        mut devices: Vec<Record>,
        mut issues: Vec<Issue>,
    ) -> (Vec<Record>, Vec<Issue>, super::support::SupportStatus) {
        let status = self.support.apply(&mut devices, &mut issues);
        (devices, issues, status)
    }
}

fn compose(slots: &[ProviderSlot]) -> (Vec<Record>, bool) {
    let mut devices = Vec::new();
    let mut has_provider_result = false;
    for records in slots.iter().filter_map(|slot| slot.last_good.as_ref()) {
        has_provider_result = true;
        devices.extend(records.iter().cloned());
    }
    (devices, has_provider_result)
}

/// Run every provider once, in parallel. A failed provider contributes an
/// issue and keeps only its own last-good records.
fn discover_all(slots: &mut [ProviderSlot]) -> Vec<Issue> {
    let outcomes: Vec<Result<Vec<Record>, ProviderError>> = thread::scope(|scope| {
        let handles: Vec<_> = slots
            .iter_mut()
            .map(|slot| scope.spawn(move || slot.provider.discover()))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    Err(ProviderError::new(
                        CODE_DISCOVERY_FAILED,
                        "The provider panicked.",
                    ))
                })
            })
            .collect()
    });

    let mut issues = Vec::new();
    for (slot, outcome) in slots.iter_mut().zip(outcomes) {
        match outcome.and_then(|records| validate(slot.name(), records)) {
            Ok(records) => slot.last_good = Some(records),
            Err(error) => {
                let retained = slot.last_good.is_some();
                issues.push(provider_issue(slot.name(), error, retained));
            }
        }
    }
    issues
}

/// The checks every provider's output must pass, in the daemon and in
/// `--test-provider`.
pub(crate) fn validate(provider: &str, records: Vec<Record>) -> Result<Vec<Record>, ProviderError> {
    let invalid =
        |reason: String| ProviderError::new("peripherals.invalid_provider_result", reason);
    let mut ids = BTreeSet::new();
    for record in &records {
        if record.id.is_empty()
            || !valid_type_token(&record.kind)
            || record.provider.is_empty()
            || !record.details.is_object()
        {
            return Err(invalid(
                "provider returned a record without a non-empty id, valid type token, provider, and details object".into(),
            ));
        }
        if record.provider != provider {
            return Err(invalid(format!(
                "provider returned a record owned by {}",
                record.provider
            )));
        }
        if !ids.insert(record.id.as_str()) {
            return Err(invalid(format!(
                "provider returned duplicate peripheral identity: {}",
                record.id
            )));
        }
    }
    Ok(records)
}

/// Type names become JSON keys beside `id`, `type` and `provider`.
pub fn valid_type_token(kind: &str) -> bool {
    let mut chars = kind.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    kind.len() <= 64
        && !matches!(kind, "id" | "type" | "provider")
        && first.is_ascii_lowercase()
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

fn provider_issue(provider: &str, error: ProviderError, retained_last_good: bool) -> Issue {
    Issue {
        provider: provider.into(),
        code: if error.code.is_empty() {
            CODE_DISCOVERY_FAILED.into()
        } else {
            error.code
        },
        reason: if error.reason.is_empty() {
            "The provider failed without reporting a reason.".into()
        } else {
            error.reason
        },
        retained_last_good,
    }
}

/// The fake provider shared by the scan and service tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A provider named `.0` whose scans return what `.1` does.
    pub(crate) struct Fake<F>(pub &'static str, pub F);

    impl<F: FnMut() -> Result<Vec<Record>, ProviderError> + Send> Provider for Fake<F> {
        fn name(&self) -> &str {
            self.0
        }
        fn subsystems(&self) -> &[String] {
            &[]
        }
        fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
            (self.1)()
        }
    }

    pub(crate) fn record(provider: &str, id: &str, details: serde_json::Value) -> Record {
        let (id, kind, provider) = (id.into(), "camera".into(), provider.into());
        Record {
            id,
            kind,
            provider,
            details,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{record, Fake};
    use super::*;
    use crate::peripherals::model::CatalogDocument;
    use crate::peripherals::support::core_rules;
    use crate::peripherals::sysutil::testing::TempDir;
    use serde_json::json;
    use std::fs;

    type Outcome = Result<Vec<Record>, ProviderError>;
    type Scanned = (CatalogDocument, Result<(), String>);

    /// A provider that returns `script` in order, then nothing.
    fn scripted(name: &'static str, script: Vec<Outcome>) -> Box<dyn Provider> {
        let mut script = script.into_iter();
        Box::new(Fake(name, move || script.next().unwrap_or(Ok(Vec::new()))))
    }

    fn found(provider: &str, ids: &[&str]) -> Outcome {
        let record = |id: &&str| record(provider, id, json!({}));
        Ok(ids.iter().map(record).collect())
    }

    fn denied() -> Outcome {
        Err(ProviderError::new("io.permission_denied", "denied"))
    }

    /// The catalog after `scans` scans of `providers`, and the last result.
    fn scan(providers: Vec<Box<dyn Provider>>, scans: usize) -> Scanned {
        let support = SupportStage::new("/nonexistent/sentinel/neat-core.json");
        let (mut scanner, mut catalog) = (Scanner::new(providers, support), Catalog::new("i", 16));
        let mut result = Ok(());
        for _ in 0..scans {
            result = scanner.scan(&mut catalog);
        }
        (catalog.document(), result)
    }

    /// `provider code [retained]` of each issue.
    fn issues(document: &CatalogDocument) -> String {
        let issue = |i: &Issue| {
            let kept = i.retained_last_good.then_some(" retained");
            format!("{} {}{}", i.provider, i.code, kept.unwrap_or_default())
        };
        Vec::from_iter(document.issues.iter().map(issue)).join(", ")
    }

    /// A failed provider keeps only its own last-good records and marks the
    /// catalog stale; a panic is a failure too. A catalog whose providers
    /// have all only failed is not ready; one without providers is.
    #[test]
    fn providers_fail_independently() {
        let panics = || -> Outcome { panic!("scripted panic") };
        let providers: Vec<Box<dyn Provider>> = vec![
            scripted("mipi", vec![found("mipi", &["m1"]), denied()]),
            scripted("usb", vec![found("usb", &[]), found("usb", &["u1"])]),
            scripted("never", vec![denied(), denied()]),
            Box::new(Fake("panics", panics)),
        ];
        let (document, result) = scan(providers, 2);
        result.unwrap();
        let ids = Vec::from_iter(document.devices.iter().map(|device| &device["id"]));
        assert_eq!(ids, ["m1", "u1"]);
        assert!(document.ready && document.stale);
        let expected = "mipi io.permission_denied retained, never io.permission_denied, \
                        panics peripherals.discovery_failed";
        assert_eq!(issues(&document), expected);
        assert!(!scan(vec![scripted("never", vec![denied()])], 1).0.ready);
        let (empty, _) = scan(vec![], 1);
        assert_eq!((empty.state.as_str(), empty.devices.len()), ("ready", 0));
    }

    /// One provider's invalid records are its own issue; ids that collide
    /// across providers reject the scan, which still counts.
    #[test]
    fn invalid_and_colliding_results_are_rejected() {
        let mut bad_type = record("usb", "y", json!({}));
        bad_type.kind = "Camera".into();
        let invalid = [
            (found("other", &["x"]), "owned by other"),
            (Ok(vec![bad_type]), "valid type token"),
            (found("usb", &["d", "d"]), "identity: d"),
        ];
        for (records, reason) in invalid {
            let (document, _) = scan(vec![scripted("usb", vec![records])], 1);
            let issue = &document.issues[0];
            assert_eq!(issue.code, "peripherals.invalid_provider_result");
            assert!(issue.reason.contains(reason), "{}", issue.reason);
        }
        let same = |name| scripted(name, vec![found(name, &["same"])]);
        let (document, result) = scan(vec![same("a"), same("b")], 1);
        assert!(result.is_err());
        assert_eq!(document.scan_sequence, 1, "refresh targets still advance");
        let expected = "catalog peripherals.invalid_provider_result";
        assert_eq!(issues(&document), expected);
        assert!(document.issues[0].reason.contains("same"));

        for good in ["camera", "microphone", "lidar_2d", "imu-6dof"] {
            assert!(valid_type_token(good), "{good}");
        }
        for bad in ["", "Camera", "1cam", "id", "type", "provider", "cam era"] {
            assert!(!valid_type_token(bad), "{bad}");
        }
    }

    /// A rejected composed result retains both the previous devices and the
    /// support metadata that classified them, even when the rules changed.
    #[test]
    fn rejected_catalog_does_not_commit_support_metadata() {
        let root = TempDir::new();
        let rules_path = root.path().join("neat-core.json");
        fs::write(&rules_path, core_rules().to_string()).unwrap();
        let camera = |provider: &str, id: &str| {
            let details = json!({
                "backend": "mipi",
                "modes": [{
                    "format": "NV12", "width": 1920, "height": 1080,
                    "framerate_num": 30, "framerate_den": 1, "isp_output": true
                }]
            });
            Ok(vec![record(provider, id, details)])
        };
        let providers = vec![
            scripted(
                "a",
                vec![camera("a", "a:1"), camera("a", "same"), camera("a", "same")],
            ),
            scripted(
                "b",
                vec![camera("b", "b:1"), camera("b", "same"), camera("b", "same")],
            ),
        ];
        let support = SupportStage::new(&rules_path);
        let (mut scanner, mut catalog) = (Scanner::new(providers, support), Catalog::new("i", 16));
        scanner.scan(&mut catalog).unwrap();
        assert!(scanner.scan(&mut catalog).is_err());
        let rejected = catalog.document();

        let mut changed = core_rules();
        changed["source"] = json!("neat-core changed");
        changed["camera"]["formats"]["accept"] = json!([]);
        fs::write(&rules_path, changed.to_string()).unwrap();
        assert!(scanner.scan(&mut catalog).is_err());
        let retained = catalog.document();
        assert_eq!(retained.revision, rejected.revision);
        assert_eq!(retained.support, rejected.support);
        assert_eq!(retained.devices, rejected.devices);
        assert_eq!(
            retained.support.unwrap().source.as_deref(),
            Some("neat-core 0.4.0")
        );
        assert_eq!(retained.devices[0]["camera"]["modes"][0]["supported"], true);
    }
}
