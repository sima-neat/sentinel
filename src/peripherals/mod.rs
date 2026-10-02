//! Event-driven peripheral catalog. A dedicated thread waits for kernel
//! uevents, runs read-only discovery providers, and publishes
//! `peripherals.json`; the API and CLI only read that file.

pub mod catalog;
pub mod cli;
pub mod external;
pub mod mipi;
pub mod model;
pub mod scan;
pub mod service;
pub mod support;
pub mod uevent;
pub mod v4l2;

use std::path::{Path, PathBuf};

use anyhow::Result;

use external::{load_providers, RejectedManifest, Trust};
use model::Provider;
use service::{Config, PeripheralsHandle};

/// Daemon settings for peripheral discovery.
pub struct Settings {
    pub catalog_path: PathBuf,
    pub providers_dir: PathBuf,
    pub support_rules_path: PathBuf,
}

/// Assemble the built-in and external providers and start the thread.
pub fn start(settings: &Settings) -> Result<PeripheralsHandle> {
    let mut providers: Vec<Box<dyn Provider>> = vec![Box::new(v4l2::V4l2Provider::new())];
    let (external, rejected) = load_providers(&settings.providers_dir, Trust::root());
    for provider in external {
        providers.push(Box::new(provider));
    }
    for (path, reason) in rejected {
        eprintln!(
            "Sentinel ignored peripheral provider {}: {reason}",
            path.display()
        );
        providers.push(Box::new(RejectedManifest::new(&path, reason)));
    }
    service::spawn(
        Config {
            catalog_path: settings.catalog_path.clone(),
            support_rules_path: settings.support_rules_path.clone(),
            instance_id: service::new_instance_id(),
            debounce: service::DEBOUNCE,
        },
        providers,
    )
}

pub fn default_settings() -> Settings {
    Settings {
        catalog_path: Path::new(model::DEFAULT_CATALOG_PATH).to_path_buf(),
        providers_dir: Path::new(model::DEFAULT_PROVIDERS_DIR).to_path_buf(),
        support_rules_path: Path::new(support::DEFAULT_RULES_PATH).to_path_buf(),
    }
}
