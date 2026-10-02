//! Event-driven peripheral catalog. A dedicated thread waits for kernel
//! uevents, runs read-only discovery providers, and publishes
//! `peripherals.json`; the API and CLI only read that file.

pub mod catalog;
pub mod external;
pub mod model;
pub mod scan;
pub mod service;
pub mod uevent;
