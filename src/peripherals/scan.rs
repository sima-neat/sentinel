use std::collections::{BTreeMap, BTreeSet};
use std::thread;

use super::catalog::Catalog;
use super::model::{Issue, Provider, ProviderError, Record};
use super::support::SupportStage;

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

struct ScanResult {
    discovered: Vec<Record>,
    accepted: bool,
    issue: Option<Issue>,
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
            let (devices, issues) = self.classify(devices, issues, catalog);
            catalog.apply_success(devices, issues)
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
        let (devices, issues) = self.classify(devices, self.provider_issues.clone(), catalog);
        catalog.apply_reclassification(devices, issues)
    }

    fn classify(
        &mut self,
        mut devices: Vec<Record>,
        mut issues: Vec<Issue>,
        catalog: &mut Catalog,
    ) -> (Vec<Record>, Vec<Issue>) {
        let status = self.support.apply(&mut devices, &mut issues);
        catalog.set_support(status);
        (devices, issues)
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
    let mut established_owners = BTreeMap::<String, String>::new();
    for slot in slots.iter() {
        for record in slot.last_good.iter().flatten() {
            established_owners
                .entry(record.id.clone())
                .or_insert_with(|| slot.name().to_string());
        }
    }

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
                        "peripherals.discovery_failed",
                        "The provider panicked.",
                    ))
                })
            })
            .collect()
    });

    let mut results: Vec<ScanResult> = slots
        .iter()
        .zip(outcomes)
        .map(|(slot, outcome)| {
            let retained = slot.last_good.is_some();
            match outcome.and_then(|records| validate(slot.name(), records)) {
                Ok(discovered) => ScanResult {
                    discovered,
                    accepted: true,
                    issue: None,
                },
                Err(error) => ScanResult {
                    discovered: Vec::new(),
                    accepted: false,
                    issue: Some(provider_issue(slot.name(), error, retained)),
                },
            }
        })
        .collect();

    resolve_ownership(slots, &mut results, &established_owners);

    let mut issues = Vec::new();
    for (slot, result) in slots.iter_mut().zip(results) {
        if result.accepted {
            slot.last_good = Some(result.discovered);
        }
        if let Some(issue) = result.issue {
            issues.push(issue);
        }
    }
    issues
}

/// Two providers may not publish the same identity. The provider that already
/// owned it keeps it; without an owner, every claimant is rejected.
fn resolve_ownership(
    slots: &[ProviderSlot],
    results: &mut [ScanResult],
    established_owners: &BTreeMap<String, String>,
) {
    loop {
        let mut claims = BTreeMap::<String, Vec<usize>>::new();
        for (index, (slot, result)) in slots.iter().zip(results.iter()).enumerate() {
            let records = if result.accepted {
                Some(&result.discovered)
            } else {
                slot.last_good.as_ref()
            };
            for record in records.into_iter().flatten() {
                claims.entry(record.id.clone()).or_default().push(index);
            }
        }
        let mut changed = false;
        for (id, claimants) in claims.iter().filter(|(_, claimants)| claimants.len() > 1) {
            let owner = established_owners.get(id).and_then(|owner| {
                claimants
                    .iter()
                    .copied()
                    .find(|&index| slots[index].name() == owner)
            });
            for &index in claimants {
                if !results[index].accepted || owner == Some(index) {
                    continue;
                }
                let reason = match owner {
                    Some(owner) => format!(
                        "peripheral identity {id} remains owned by provider {}",
                        slots[owner].name()
                    ),
                    None => format!(
                        "peripheral identity {id} was returned by multiple providers without an established owner"
                    ),
                };
                results[index].accepted = false;
                results[index].issue = Some(provider_issue(
                    slots[index].name(),
                    ProviderError::new("peripherals.invalid_provider_result", reason),
                    slots[index].last_good.is_some(),
                ));
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
}

fn validate(provider: &str, records: Vec<Record>) -> Result<Vec<Record>, ProviderError> {
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
            "peripherals.discovery_failed".into()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;

    struct Scripted {
        name: String,
        subsystems: Vec<String>,
        script: VecDeque<Result<Vec<Record>, ProviderError>>,
    }

    impl Scripted {
        fn boxed(name: &str, script: Vec<Result<Vec<Record>, ProviderError>>) -> Box<dyn Provider> {
            Box::new(Self {
                name: name.into(),
                subsystems: vec!["video4linux".into()],
                script: script.into(),
            })
        }
    }

    impl Provider for Scripted {
        fn name(&self) -> &str {
            &self.name
        }
        fn subsystems(&self) -> &[String] {
            &self.subsystems
        }
        fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
            if self.name == "panics" {
                panic!("scripted panic");
            }
            self.script.pop_front().unwrap_or(Ok(Vec::new()))
        }
    }

    fn record(provider: &str, id: &str) -> Record {
        Record {
            id: id.into(),
            kind: "camera".into(),
            provider: provider.into(),
            details: json!({}),
        }
    }

    fn denied() -> ProviderError {
        ProviderError::new("io.permission_denied", "denied")
    }

    fn scanner(providers: Vec<Box<dyn Provider>>) -> Scanner {
        Scanner::new(
            providers,
            SupportStage::new("/nonexistent/sentinel/neat-core.json"),
        )
    }

    fn ids(catalog: &Catalog) -> Vec<String> {
        catalog
            .document()
            .devices
            .iter()
            .map(|device| device["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn failed_provider_keeps_its_last_good_records_beside_healthy_ones() {
        let mut scanner = scanner(vec![
            Scripted::boxed("mipi", vec![Ok(vec![record("mipi", "m1")]), Err(denied())]),
            Scripted::boxed("usb", vec![Ok(vec![]), Ok(vec![record("usb", "u1")])]),
        ]);
        let mut catalog = Catalog::new("i", 16);
        scanner.scan(&mut catalog).unwrap();
        scanner.scan(&mut catalog).unwrap();

        assert_eq!(ids(&catalog), vec!["m1", "u1"]);
        let document = catalog.document();
        assert!(document.stale);
        assert_eq!(document.issues.len(), 1);
        assert_eq!(document.issues[0].provider, "mipi");
        assert!(document.issues[0].retained_last_good);
    }

    #[test]
    fn provider_failing_before_any_success_leaves_catalog_unready_until_another_succeeds() {
        let mut scanner = scanner(vec![Scripted::boxed("mipi", vec![Err(denied())])]);
        let mut catalog = Catalog::new("i", 16);
        scanner.scan(&mut catalog).unwrap();
        let document = catalog.document();
        assert!(!document.ready);
        assert!(!document.issues[0].retained_last_good);
    }

    #[test]
    fn panicking_provider_becomes_an_issue() {
        let mut scanner = scanner(vec![
            Scripted::boxed("panics", vec![]),
            Scripted::boxed("usb", vec![Ok(vec![record("usb", "u1")])]),
        ]);
        let mut catalog = Catalog::new("i", 16);
        scanner.scan(&mut catalog).unwrap();
        let document = catalog.document();
        assert_eq!(ids(&catalog), vec!["u1"]);
        assert_eq!(document.issues[0].provider, "panics");
        assert_eq!(document.issues[0].code, "peripherals.discovery_failed");
    }

    #[test]
    fn invalid_records_are_rejected_per_provider() {
        let mut wrong_owner = record("other", "x");
        wrong_owner.provider = "other".into();
        let mut bad_type = record("usb", "y");
        bad_type.kind = "Camera".into();
        for invalid in [
            vec![wrong_owner],
            vec![bad_type],
            vec![record("usb", "d"), record("usb", "d")],
        ] {
            let mut scanner = scanner(vec![Scripted::boxed("usb", vec![Ok(invalid)])]);
            let mut catalog = Catalog::new("i", 16);
            scanner.scan(&mut catalog).unwrap();
            assert_eq!(
                catalog.document().issues[0].code,
                "peripherals.invalid_provider_result"
            );
        }
    }

    #[test]
    fn established_owner_keeps_a_contested_identity() {
        let mut scanner = scanner(vec![
            Scripted::boxed(
                "mipi",
                vec![
                    Ok(vec![record("mipi", "same")]),
                    Ok(vec![record("mipi", "same")]),
                ],
            ),
            Scripted::boxed("usb", vec![Ok(vec![]), Ok(vec![record("usb", "same")])]),
        ]);
        let mut catalog = Catalog::new("i", 16);
        scanner.scan(&mut catalog).unwrap();
        scanner.scan(&mut catalog).unwrap();
        let document = catalog.document();
        assert_eq!(ids(&catalog), vec!["same"]);
        assert_eq!(document.devices[0]["provider"], "mipi");
        assert_eq!(document.issues[0].provider, "usb");
        assert!(document.issues[0]
            .reason
            .contains("remains owned by provider mipi"));
    }

    #[test]
    fn unowned_contested_identity_rejects_every_claimant() {
        let mut scanner = scanner(vec![
            Scripted::boxed("a", vec![Ok(vec![record("a", "same")])]),
            Scripted::boxed("b", vec![Ok(vec![record("b", "same")])]),
        ]);
        let mut catalog = Catalog::new("i", 16);
        scanner.scan(&mut catalog).unwrap();
        let document = catalog.document();
        assert!(document.devices.is_empty());
        assert_eq!(document.issues.len(), 2);
    }

    #[test]
    fn no_providers_publish_a_ready_empty_catalog() {
        let mut catalog = Catalog::new("i", 16);
        scanner(vec![]).scan(&mut catalog).unwrap();
        let document = catalog.document();
        assert!(document.ready);
        assert_eq!(document.state, "ready");
        assert!(document.devices.is_empty());
    }

    #[test]
    fn type_tokens_follow_the_json_key_rules() {
        for good in ["camera", "microphone", "lidar_2d", "imu-6dof"] {
            assert!(valid_type_token(good), "{good}");
        }
        for bad in ["", "Camera", "1cam", "id", "type", "provider", "cam era"] {
            assert!(!valid_type_token(bad), "{bad}");
        }
    }
}
