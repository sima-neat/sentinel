//! Event-driven peripheral catalog. A dedicated thread waits for kernel
//! uevents, runs read-only discovery providers, and publishes
//! `peripherals.json`; the API and CLI only read that file.

pub mod catalog;
pub mod cli;
pub mod mipi;
pub mod model;
pub mod scan;
pub mod service;
pub mod support;
mod sysutil;
pub mod uevent;
pub mod v4l2;
mod videodev2;

use std::path::PathBuf;

use anyhow::Result;

use model::Provider;
use service::{Config, PeripheralsHandle};

/// Daemon settings for peripheral discovery.
pub struct Settings {
    pub catalog_path: PathBuf,
    pub support_rules_path: PathBuf,
}

/// Every built-in provider. To add a device type that the kernel can
/// describe, add its provider here (see docs/peripherals).
pub fn builtin_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(mipi::MipiProvider::new()),
        Box::new(v4l2::V4l2Provider::new()),
    ]
}

/// Start the peripherals thread with the built-in providers.
pub fn start(settings: &Settings) -> Result<PeripheralsHandle> {
    service::spawn(
        Config {
            catalog_path: settings.catalog_path.clone(),
            support_rules_path: settings.support_rules_path.clone(),
            instance_id: service::new_instance_id(),
            debounce: service::DEBOUNCE,
            listen_for_uevents: true,
        },
        builtin_providers(),
    )
}

pub fn default_settings() -> Settings {
    Settings {
        catalog_path: PathBuf::from(model::DEFAULT_CATALOG_PATH),
        support_rules_path: PathBuf::from(support::DEFAULT_RULES_PATH),
    }
}
