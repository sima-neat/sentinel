use std::collections::{BTreeMap, BTreeSet};
use std::thread;

use super::catalog::Catalog;
use super::model::{Issue, Provider, ProviderError, Record};

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

    pub fn subsystems(&self) -> &[String] {
        self.provider.subsystems()
    }
}

struct ScanResult {
    discovered: Vec<Record>,
    accepted: bool,
    issue: Option<Issue>,
}

/// Run every provider once, in parallel, and apply the combined result. A
/// failed provider contributes an issue and keeps only its own last-good
/// records; healthy providers are always published.
pub fn run_scan(slots: &mut [ProviderSlot], catalog: &mut Catalog) -> Result<(), String> {
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

    let mut devices = Vec::new();
    let mut issues = Vec::new();
    let mut has_provider_result = false;
    for (slot, result) in slots.iter_mut().zip(results) {
        if result.accepted {
            slot.last_good = Some(result.discovered);
        }
        if let Some(issue) = result.issue {
            issues.push(issue);
        }
        if let Some(records) = &slot.last_good {
            has_provider_result = true;
            devices.extend(records.iter().cloned());
        }
    }
    if has_provider_result {
        catalog.apply_success(devices, issues)
    } else {
        catalog.apply_provider_failure(issues)
    }
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
        let mut slots = vec![
            ProviderSlot::new(Scripted::boxed(
                "mipi",
                vec![Ok(vec![record("mipi", "m1")]), Err(denied())],
            )),
            ProviderSlot::new(Scripted::boxed(
                "usb",
                vec![Ok(vec![]), Ok(vec![record("usb", "u1")])],
            )),
        ];
        let mut catalog = Catalog::new("i", 16);
        run_scan(&mut slots, &mut catalog).unwrap();
        run_scan(&mut slots, &mut catalog).unwrap();

        assert_eq!(ids(&catalog), vec!["m1", "u1"]);
        let document = catalog.document();
        assert!(document.stale);
        assert_eq!(document.issues.len(), 1);
        assert_eq!(document.issues[0].provider, "mipi");
        assert!(document.issues[0].retained_last_good);
    }

    #[test]
    fn provider_failing_before_any_success_leaves_catalog_unready_until_another_succeeds() {
        let mut slots = vec![ProviderSlot::new(Scripted::boxed(
            "mipi",
            vec![Err(denied())],
        ))];
        let mut catalog = Catalog::new("i", 16);
        run_scan(&mut slots, &mut catalog).unwrap();
        let document = catalog.document();
        assert!(!document.ready);
        assert!(!document.issues[0].retained_last_good);
    }

    #[test]
    fn panicking_provider_becomes_an_issue() {
        let mut slots = vec![
            ProviderSlot::new(Scripted::boxed("panics", vec![])),
            ProviderSlot::new(Scripted::boxed("usb", vec![Ok(vec![record("usb", "u1")])])),
        ];
        let mut catalog = Catalog::new("i", 16);
        run_scan(&mut slots, &mut catalog).unwrap();
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
            let mut slots = vec![ProviderSlot::new(Scripted::boxed("usb", vec![Ok(invalid)]))];
            let mut catalog = Catalog::new("i", 16);
            run_scan(&mut slots, &mut catalog).unwrap();
            assert_eq!(
                catalog.document().issues[0].code,
                "peripherals.invalid_provider_result"
            );
        }
    }

    #[test]
    fn established_owner_keeps_a_contested_identity() {
        let mut slots = vec![
            ProviderSlot::new(Scripted::boxed(
                "mipi",
                vec![
                    Ok(vec![record("mipi", "same")]),
                    Ok(vec![record("mipi", "same")]),
                ],
            )),
            ProviderSlot::new(Scripted::boxed(
                "usb",
                vec![Ok(vec![]), Ok(vec![record("usb", "same")])],
            )),
        ];
        let mut catalog = Catalog::new("i", 16);
        run_scan(&mut slots, &mut catalog).unwrap();
        run_scan(&mut slots, &mut catalog).unwrap();
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
        let mut slots = vec![
            ProviderSlot::new(Scripted::boxed("a", vec![Ok(vec![record("a", "same")])])),
            ProviderSlot::new(Scripted::boxed("b", vec![Ok(vec![record("b", "same")])])),
        ];
        let mut catalog = Catalog::new("i", 16);
        run_scan(&mut slots, &mut catalog).unwrap();
        let document = catalog.document();
        assert!(document.devices.is_empty());
        assert_eq!(document.issues.len(), 2);
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
