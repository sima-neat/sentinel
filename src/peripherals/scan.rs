use std::collections::BTreeSet;
use std::path::Path;
use std::thread;

use super::catalog::Catalog;
use super::model::{Issue, Provider, ProviderError, Record};
use super::support::SupportStage;

/// A provider plus the records it returned in its last successful scan.
struct Slot {
    provider: Box<dyn Provider>,
    last_good: Option<Vec<Record>>,
}

/// Owns the providers, their last-good records, and the support stage.
pub struct Scanner {
    slots: Vec<Slot>,
    provider_issues: Vec<Issue>,
    support: SupportStage,
}

impl Scanner {
    pub fn new(providers: Vec<Box<dyn Provider>>, support: SupportStage) -> Self {
        let slots = providers
            .into_iter()
            .map(|provider| Slot {
                provider,
                last_good: None,
            })
            .collect();
        Self {
            slots,
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

    pub fn support_path(&self) -> &Path {
        self.support.path()
    }

    /// Run every provider in parallel, apply the support rules, then publish
    /// to the catalog. A failed provider becomes an issue and keeps only its
    /// own last-good records.
    pub fn scan(&mut self, catalog: &mut Catalog) -> Result<(), String> {
        let outcomes: Vec<_> = thread::scope(|scope| {
            let handles: Vec<_> = self
                .slots
                .iter_mut()
                .map(|slot| scope.spawn(move || slot.provider.discover()))
                .collect();
            handles.into_iter().map(|handle| handle.join()).collect()
        });
        self.provider_issues.clear();
        for (slot, outcome) in self.slots.iter_mut().zip(outcomes) {
            let name = slot.provider.name();
            let outcome = outcome
                .unwrap_or_else(|_| {
                    Err(ProviderError::new(
                        "peripherals.discovery_failed",
                        "The provider panicked.",
                    ))
                })
                .and_then(|records| validate(name, records));
            match outcome {
                Ok(records) => slot.last_good = Some(records),
                Err(error) => self.provider_issues.push(Issue {
                    provider: name.into(),
                    code: error.code,
                    reason: error.reason,
                    retained_last_good: slot.last_good.is_some(),
                }),
            }
        }
        let Some(devices) = self.last_good() else {
            // No usable records yet; zero providers is a ready, empty catalog.
            if !self.provider_issues.is_empty() {
                catalog.apply_issues_only(self.provider_issues.clone());
                return Ok(());
            }
            return catalog.apply_success(Vec::new(), Vec::new());
        };
        let (devices, issues) = self.classify(devices, catalog);
        // Providers own distinct id prefixes, so a collision is a provider bug.
        // It still counts as a scan so refresh targets are reached.
        catalog
            .apply_success(devices, issues)
            .inspect_err(|reason| {
                catalog.apply_issues_only(vec![Issue {
                    provider: "catalog".into(),
                    code: "peripherals.invalid_provider_result".into(),
                    reason: reason.clone(),
                    retained_last_good: true,
                }]);
            })
    }

    /// Re-apply changed support rules to the last scan's records without
    /// reading any hardware.
    pub fn reclassify(&mut self, catalog: &mut Catalog) -> Result<(), String> {
        let Some(devices) = self.last_good() else {
            return Ok(());
        };
        let (devices, issues) = self.classify(devices, catalog);
        catalog.apply_reclassification(devices, issues)
    }

    fn last_good(&self) -> Option<Vec<Record>> {
        let mut records = self
            .slots
            .iter()
            .filter_map(|slot| slot.last_good.as_ref())
            .peekable();
        records.peek()?;
        Some(records.flatten().cloned().collect())
    }

    fn classify(
        &mut self,
        mut devices: Vec<Record>,
        catalog: &mut Catalog,
    ) -> (Vec<Record>, Vec<Issue>) {
        let mut issues = self.provider_issues.clone();
        catalog.set_support(self.support.apply(&mut devices, &mut issues));
        (devices, issues)
    }
}

/// The checks every provider's output must pass, in the daemon and in
/// `--test-provider`.
pub(crate) fn validate(provider: &str, records: Vec<Record>) -> Result<Vec<Record>, ProviderError> {
    let mut ids = BTreeSet::new();
    let problem = records.iter().find_map(|record| {
        if record.id.is_empty() || !valid_type_token(&record.kind) || !record.details.is_object() {
            Some(format!(
                "{provider} returned a record without an id, a valid type, or a details object"
            ))
        } else if record.provider != provider {
            Some(format!(
                "{provider} returned a record owned by {}",
                record.provider
            ))
        } else if !ids.insert(record.id.as_str()) {
            Some(format!(
                "{provider} returned duplicate peripheral identity: {}",
                record.id
            ))
        } else {
            None
        }
    });
    match problem {
        Some(reason) => Err(ProviderError::new(
            "peripherals.invalid_provider_result",
            reason,
        )),
        None => Ok(records),
    }
}

/// Type names become JSON keys beside `id`, `type` and `provider`.
fn valid_type_token(kind: &str) -> bool {
    kind.len() <= 64
        && kind.starts_with(|ch: char| ch.is_ascii_lowercase())
        && !matches!(kind, "id" | "type" | "provider")
        && kind
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;

    /// Returns one scripted result per scan; `panics` panics instead.
    struct Scripted(&'static str, VecDeque<Result<Vec<Record>, ProviderError>>);

    impl Provider for Scripted {
        fn name(&self) -> &str {
            self.0
        }
        fn subsystems(&self) -> &[String] {
            &[]
        }
        fn discover(&mut self) -> Result<Vec<Record>, ProviderError> {
            assert_ne!(self.0, "panics", "scripted panic");
            self.1.pop_front().unwrap_or(Ok(Vec::new()))
        }
    }

    fn provider(
        name: &'static str,
        script: Vec<Result<Vec<Record>, ProviderError>>,
    ) -> Box<dyn Provider> {
        Box::new(Scripted(name, script.into()))
    }

    fn record(provider: &str, id: &str) -> Record {
        Record {
            id: id.into(),
            kind: "camera".into(),
            provider: provider.into(),
            details: json!({}),
        }
    }

    fn scan(providers: Vec<Box<dyn Provider>>, scans: usize) -> (Catalog, Result<(), String>) {
        let mut scanner = Scanner::new(providers, SupportStage::new("/nonexistent/neat-core.json"));
        let mut catalog = Catalog::new("i");
        let mut result = Ok(());
        for _ in 0..scans {
            result = scanner.scan(&mut catalog);
        }
        (catalog, result)
    }

    fn ids(catalog: &Catalog) -> Vec<String> {
        let devices = catalog.document().devices;
        devices
            .iter()
            .map(|device| device["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_failed_provider_keeps_its_last_good_records_beside_healthy_ones() {
        let denied = ProviderError::new("io.permission_denied", "denied");
        let (catalog, _) = scan(
            vec![
                provider("mipi", vec![Ok(vec![record("mipi", "m1")]), Err(denied)]),
                provider("usb", vec![Ok(vec![]), Ok(vec![record("usb", "u1")])]),
                provider("panics", vec![]),
            ],
            2,
        );
        let document = catalog.document();
        assert_eq!(ids(&catalog), ["m1", "u1"]);
        let issues: Vec<_> = document
            .issues
            .iter()
            .map(|issue| (issue.provider.as_str(), issue.retained_last_good))
            .collect();
        assert_eq!(issues, [("mipi", true), ("panics", false)]);
        assert!(document.stale);
    }

    #[test]
    fn invalid_records_are_rejected_per_provider() {
        let mut bad_type = record("usb", "y");
        bad_type.kind = "Camera".into();
        for invalid in [
            vec![record("other", "x")],
            vec![bad_type],
            vec![record("usb", "d"), record("usb", "d")],
        ] {
            let (catalog, _) = scan(vec![provider("usb", vec![Ok(invalid)])], 1);
            assert!(!catalog.document().ready);
            assert_eq!(
                catalog.document().issues[0].code,
                "peripherals.invalid_provider_result"
            );
        }
    }

    #[test]
    fn colliding_ids_reject_the_scan_but_count_it() {
        let (catalog, result) = scan(
            vec![
                provider("a", vec![Ok(vec![record("a", "same")])]),
                provider("b", vec![Ok(vec![record("b", "same")])]),
            ],
            1,
        );
        assert!(result.is_err());
        let document = catalog.document();
        assert_eq!(
            (document.scan_sequence, document.issues[0].provider.as_str()),
            (1, "catalog")
        );
    }

    #[test]
    fn no_providers_publish_a_ready_empty_catalog() {
        let (catalog, _) = scan(vec![], 1);
        assert_eq!(catalog.document().state, "ready");
    }
}
