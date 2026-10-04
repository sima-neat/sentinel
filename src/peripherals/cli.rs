use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::model::CatalogDocument;
use super::scan::validate;
use super::service::read;
use super::support::SupportStage;
use super::{builtin_providers, Settings};

const CATALOG_UNAVAILABLE: &str =
    "peripheral catalog unavailable; check `systemctl status simaai-sentinel`";

/// `simaai-sentinel peripherals [--json] [--refresh] [--test-provider NAME]`
pub fn run(settings: &Settings, api_socket: &Path, args: &[String]) -> Result<()> {
    let catalog_path = settings.catalog_path.as_path();
    let mut json = false;
    let mut refresh = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "--refresh" => refresh = true,
            "--test-provider" => {
                let target = args
                    .get(index + 1)
                    .context("--test-provider needs a provider name, e.g. daemon.camera.mipi")?;
                if let Some(extra) = args.get(index + 2) {
                    bail!("--test-provider takes no further options (got '{extra}')");
                }
                return test_provider(target, settings);
            }
            other => bail!("unknown peripherals option '{other}'"),
        }
        index += 1;
    }
    if refresh {
        let instance = read(catalog_path)
            .map(|document| document.instance_id)
            .context(CATALOG_UNAVAILABLE)?;
        let target = request_refresh(api_socket)?;
        wait_for_scan(catalog_path, &instance, target)?;
    }
    let document = read(catalog_path).context(CATALOG_UNAVAILABLE)?;
    if !daemon_running(api_socket) {
        eprintln!(
            "Warning: the Sentinel daemon is not running, so this catalog may be out of date; \
check `systemctl status simaai-sentinel`."
        );
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else {
        print!("{}", render(&document));
    }
    Ok(())
}

/// Run one provider once, as the daemon would, and print the records it
/// would add to the catalog. Built for people adding a device type.
fn test_provider(target: &str, settings: &Settings) -> Result<()> {
    let providers = builtin_providers();
    let names: Vec<&str> = providers.iter().map(|provider| provider.name()).collect();
    let names = names.join(", ");
    let mut provider = providers
        .into_iter()
        .find(|provider| provider.name() == target)
        .with_context(|| format!("no provider '{target}'; built-in providers: {names}"))?;
    let name = provider.name().to_string();
    let outcome = provider
        .discover()
        .and_then(|records| validate(&name, records));
    let mut report = serde_json::json!({
        "provider": name,
        "subsystems": provider.subsystems(),
        "ok": outcome.is_ok(),
    });
    match outcome {
        Ok(mut records) => {
            let mut issues = Vec::new();
            let support = SupportStage::new(settings.support_rules_path.clone())
                .apply(&mut records, &mut issues);
            report["records"] = records
                .iter()
                .map(|record| record.to_catalog_value())
                .collect();
            report["support"] = serde_json::to_value(support)?;
            report["support_issues"] = serde_json::to_value(issues)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Err(error) => {
            report["error"] = serde_json::to_value(error)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            bail!("provider '{name}' failed; see \"error\" above")
        }
    }
}

/// The catalog file outlives the daemon (a stop by SIGTERM leaves it as it
/// was), so the CLI asks the daemon for its health before trusting it. A
/// complete request keeps the daemon from logging a dropped connection.
fn daemon_running(api_socket: &Path) -> bool {
    exchange(
        api_socket,
        b"GET /v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .is_ok_and(|response| response.starts_with("HTTP/1.1 "))
}

/// Send one request to the daemon and read its whole response.
fn exchange(api_socket: &Path, request: &[u8]) -> Result<String> {
    let mut stream = UnixStream::connect(api_socket)
        .with_context(|| format!("connect to {}", api_socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request)?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn request_refresh(api_socket: &Path) -> Result<u64> {
    let response = exchange(
        api_socket,
        b"POST /v1/peripherals/refresh HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
    )?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_default();
    let value: Value = serde_json::from_str(body).context("parse refresh response")?;
    value["target_scan_sequence"].as_u64().with_context(|| {
        format!(
            "refresh refused: {}",
            value["error"].as_str().unwrap_or("unexpected response")
        )
    })
}

fn wait_for_scan(catalog_path: &Path, instance: &str, target: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(document) = read(catalog_path) {
            if document.instance_id != instance {
                bail!("Sentinel restarted during the refresh; run the command again");
            }
            if document.scan_sequence >= target {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("refresh did not complete within 15 seconds")
}

/// Device names come from the hardware; strip control characters so a device
/// cannot send escape sequences to the administrator's terminal.
fn printable(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' {
                '?'
            } else {
                ch
            }
        })
        .collect()
}

fn render(document: &CatalogDocument) -> String {
    let mut out = format!(
        "Peripherals  {}  revision {}  scan {}  updated {}\n",
        document.state,
        document.revision,
        document.scan_sequence,
        document.last_attempt_at.as_deref().unwrap_or("never"),
    );
    if document.stale {
        out.push_str("  Showing last-good records for a provider that could not be refreshed.\n");
    }
    if !document.ready {
        out.push_str("  The first scan has not completed.\n");
    } else if document.devices.is_empty() {
        out.push_str("  No peripherals found.\n");
    } else {
        out.push_str(&format!(
            "\n  {:<8} {:<36} {:<26} {}\n",
            "TYPE", "ID", "PROVIDER", "NAME / MODES"
        ));
        for device in &document.devices {
            let kind = device["type"].as_str().unwrap_or("?");
            let details = &device[kind];
            let name = ["camera_name", "model", "name"]
                .iter()
                .find_map(|key| details[*key].as_str())
                .unwrap_or("-");
            let modes = details["modes"].as_array();
            let supported = modes.map_or(0, |modes| {
                modes
                    .iter()
                    .filter(|mode| mode["supported"].as_bool() == Some(true))
                    .count()
            });
            let summary = match modes {
                Some(modes) => format!("{name}  ({} modes, {supported} supported)", modes.len()),
                None => name.to_string(),
            };
            out.push_str(&format!(
                "  {:<8} {:<36} {:<26} {}\n",
                kind,
                device["id"].as_str().unwrap_or("?"),
                device["provider"].as_str().unwrap_or("?"),
                summary
            ));
        }
    }
    for issue in &document.issues {
        out.push_str(&format!(
            "  ! {} {}: {}\n",
            issue.provider, issue.code, issue.reason
        ));
    }
    if let Some(error) = &document.error {
        out.push_str(&format!(
            "  ! {}: {}\n",
            error["code"].as_str().unwrap_or("error"),
            error["reason"].as_str().unwrap_or("")
        ));
    }
    printable(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peripherals::catalog::Catalog;
    use crate::peripherals::model::Record;
    use serde_json::json;

    #[test]
    fn unknown_provider_names_list_the_built_in_ones() {
        let settings = Settings {
            catalog_path: "/nonexistent/peripherals.json".into(),
            support_rules_path: "/nonexistent/neat-core.json".into(),
        };
        let error = test_provider("no.such.provider", &settings).unwrap_err();
        assert!(error.to_string().contains("daemon.camera.mipi"), "{error}");
        assert!(error.to_string().contains("daemon.camera.v4l2"), "{error}");
    }

    #[test]
    fn device_text_cannot_inject_terminal_escapes() {
        let mut catalog = Catalog::new("i", 8);
        catalog
            .apply_success(
                vec![Record {
                    id: "camera:x".into(),
                    kind: "camera".into(),
                    provider: "p".into(),
                    details: json!({"model": "evil\u{1b}]0;owned\u{7}cam"}),
                }],
                vec![],
            )
            .unwrap();
        let text = render(&catalog.document());
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{7}'),
            "{text:?}"
        );
        assert!(text.contains("evil?]0;owned?cam"));
    }

    #[test]
    fn a_missing_or_dead_socket_means_the_daemon_is_not_running() {
        assert!(!daemon_running(Path::new("/nonexistent/sentinel/api.sock")));
        let dir = std::env::temp_dir().join(format!("sentinel-cli-socket-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("api.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // The daemon answers a complete health request, so it logs nothing.
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 256];
            let length = stream.read(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8_lossy(&request[..length]).into_owned()
        });
        assert!(daemon_running(&path));
        assert!(server
            .join()
            .unwrap()
            .starts_with("GET /v1/health HTTP/1.1\r\n"));
        assert!(
            !daemon_running(&path),
            "a stale socket file is not a running daemon"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn renders_devices_modes_and_states() {
        let mut catalog = Catalog::new("i", 8);
        assert!(render(&catalog.document()).contains("first scan has not completed"));
        catalog.apply_success(vec![], vec![]).unwrap();
        assert!(render(&catalog.document()).contains("No peripherals found"));
        catalog
            .apply_success(
                vec![Record {
                    id: "camera:imx477 5-001a".into(),
                    kind: "camera".into(),
                    provider: "daemon.camera.libcamera".into(),
                    details: json!({"camera_name": "imx477 5-001a", "modes": [
                        {"supported": true}, {"supported": false}
                    ]}),
                }],
                vec![],
            )
            .unwrap();
        let text = render(&catalog.document());
        assert!(
            text.contains("imx477 5-001a  (2 modes, 1 supported)"),
            "{text}"
        );
    }
}
