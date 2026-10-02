//! Event-driven peripheral catalog. A dedicated thread waits for kernel
//! uevents, runs read-only discovery providers, and publishes
//! `peripherals.json`; the API and CLI only read that file.

pub mod model;
