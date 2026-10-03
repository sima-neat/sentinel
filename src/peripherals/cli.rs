use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::json;

use super::model::CatalogDocument;
use super::scan::validate;
use super::service::read;
use super::support::SupportStage;
use super::{builtin_providers, Settings};

/// `simaai-sentinel peripherals [--json] [--test-provider NAME]`
pub fn run(settings: &Settings, api_socket: &Path, args: &[String]) -> Result<()> {
    match args {
        [] => {}
        [flag] if flag == "--json" => {}
        [flag, name] if flag == "--test-provider" => return test_provider(name, settings),
        _ => bail!("usage: simaai-sentinel peripherals [--json] [--test-provider NAME]"),
    }
    let document = read(&settings.catalog_path)
        .context("peripheral catalog unavailable; check `systemctl status simaai-sentinel`")?;
    // A SIGTERM stop leaves the file as it was, so check the daemon is alive.
    if UnixStream::connect(api_socket).is_err() {
        eprintln!("Warning: the Sentinel daemon is not running; this catalog may be out of date.");
    }
    if args.is_empty() {
        print!("{}", render(&document));
    } else {
        println!("{}", serde_json::to_string_pretty(&document)?);
    }
    Ok(())
}

/// Run one built-in provider once, as the daemon would, and print the records
/// it would add to the catalog. Built for people adding a device type.
fn test_provider(name: &str, settings: &Settings) -> Result<()> {
    let providers = builtin_providers();
    let names: Vec<&str> = providers.iter().map(|provider| provider.name()).collect();
    let names = names.join(", ");
    let mut provider = providers
        .into_iter()
        .find(|provider| provider.name() == name)
        .with_context(|| format!("no provider '{name}'; built-in providers: {names}"))?;
    let outcome = provider
        .discover()
        .and_then(|records| validate(name, records));
    let report = match outcome {
        Ok(mut records) => {
            let mut issues = Vec::new();
            let support =
                SupportStage::new(&settings.support_rules_path).apply(&mut records, &mut issues);
            let records: Vec<_> = records
                .iter()
                .map(|record| record.to_catalog_value())
                .collect();
            json!({"provider": name, "ok": true, "records": records, "support": support, "support_issues": issues})
        }
        Err(error) => json!({"provider": name, "ok": false, "error": error}),
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report["ok"] == false {
        bail!("provider '{name}' failed; see \"error\" above");
    }
    Ok(())
}

fn render(document: &CatalogDocument) -> String {
    let mut out = format!(
        "Peripherals  {}  revision {}  updated {}\n",
        document.state,
        document.revision,
        document.last_attempt_at.as_deref().unwrap_or("never"),
    );
    if !document.ready {
        out.push_str("  The first scan has not completed.\n");
    } else if document.devices.is_empty() {
        out.push_str("  No peripherals found.\n");
    }
    for device in &document.devices {
        let kind = device["type"].as_str().unwrap_or("?");
        let details = &device[kind];
        let name = ["camera_name", "model"]
            .iter()
            .find_map(|key| details[*key].as_str())
            .unwrap_or("-");
        let modes = details["modes"].as_array().map_or(String::new(), |modes| {
            let supported = modes
                .iter()
                .filter(|mode| mode["supported"] == true)
                .count();
            format!("  ({} modes, {supported} supported)", modes.len())
        });
        let id = device["id"].as_str().unwrap_or("?");
        out.push_str(&format!("  {kind:<8} {id:<36} {name}{modes}\n"));
    }
    for issue in &document.issues {
        out.push_str(&format!(
            "  ! {} {}: {}\n",
            issue.provider, issue.code, issue.reason
        ));
    }
    if let Some(error) = &document.error {
        out.push_str(&format!("  ! {}: {}\n", error["code"], error["reason"]));
    }
    // Device names come from hardware: never pass control characters to the
    // administrator's terminal.
    out.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' {
                '?'
            } else {
                ch
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripherals::catalog::Catalog;
    use crate::peripherals::model::Record;

    #[test]
    fn renders_cameras_and_escapes_device_text() {
        let mut catalog = Catalog::new("i");
        assert!(render(&catalog.document()).contains("first scan has not completed"));
        let camera = Record {
            id: "camera:imx477 5-001a".into(),
            kind: "camera".into(),
            provider: "daemon.camera.mipi".into(),
            details: json!({"model": "evil\u{1b}]0;x\u{7}", "modes": [{"supported": true}, {"supported": false}]}),
        };
        catalog.apply_success(vec![camera], vec![]).unwrap();
        let text = render(&catalog.document());
        assert!(
            text.contains("evil?]0;x?  (2 modes, 1 supported)"),
            "{text:?}"
        );
    }

    #[test]
    fn unknown_provider_names_list_the_built_in_ones() {
        let settings = Settings {
            catalog_path: "/nonexistent/peripherals.json".into(),
            support_rules_path: "/nonexistent/neat-core.json".into(),
        };
        let error = test_provider("no.such.provider", &settings)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("daemon.camera.mipi") && error.contains("daemon.camera.v4l2"),
            "{error}"
        );
    }
}
