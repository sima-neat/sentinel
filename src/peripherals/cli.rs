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
    if !daemon_serves_catalog(api_socket) {
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
/// was), and a daemon running without peripheral discovery leaves an old one
/// in place, so the CLI trusts it only when the daemon's health reports a
/// peripheral summary. A complete request keeps the daemon from logging a
/// dropped connection.
fn daemon_serves_catalog(api_socket: &Path) -> bool {
    let Ok(response) = exchange(
        api_socket,
        b"GET /v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n",
    ) else {
        return false;
    };
    let body = response.split_once("\r\n\r\n").map(|(_, body)| body);
    let health: Option<Value> = body.and_then(|body| serde_json::from_str(body).ok());
    health.is_some_and(|health| health["peripherals"].is_object())
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
    use crate::peripherals::model::Issue;
    use crate::peripherals::scan::testing::record;
    use crate::peripherals::sysutil::testing::TempDir;
    use serde_json::json;
    use std::os::unix::net::UnixListener;

    #[test]
    fn unknown_provider_names_list_the_built_in_ones() {
        let settings = Settings {
            catalog_path: "/nonexistent/peripherals.json".into(),
            support_rules_path: "/nonexistent/neat-core.json".into(),
        };
        let error = test_provider("no.such.provider", &settings).unwrap_err();
        let names = "built-in providers: daemon.camera.mipi, daemon.camera.v4l2";
        assert!(error.to_string().ends_with(names), "{error}");
    }

    /// Device names come from the hardware, so control characters are
    /// replaced before they reach the terminal.
    #[test]
    fn renders_devices_modes_and_states_without_terminal_escapes() {
        let mut catalog = Catalog::new("i", 8);
        let text = |catalog: &Catalog| {
            let document = catalog.document();
            let (text, updated) = (render(&document), document.last_attempt_at);
            updated.map_or(text.clone(), |at| text.replace(&at, "<time>"))
        };
        let starting = "Peripherals  starting  revision 0  scan 0  updated never\n";
        let first = "  The first scan has not completed.\n";
        assert_eq!(text(&catalog), format!("{starting}{first}"));
        catalog.apply_success(vec![], vec![]).unwrap();
        let empty = "Peripherals  ready  revision 1  scan 1  updated <time>\n";
        assert_eq!(text(&catalog), format!("{empty}  No peripherals found.\n"));
        let modes = json!([{"supported": true}, {"supported": false}]);
        let imx477 = json!({"camera_name": "imx477 5-001a", "model": "imx477", "modes": modes});
        let evil = json!({"model": "evil\u{1b}]0;owned\u{7}cam"});
        let devices = vec![record("p", "a", imx477), record("p", "b", evil)];
        let issue = json!({"provider": "p", "code": "io.open", "reason": "gone\u{1b}[2J",
                           "retained_last_good": true});
        let issue: Issue = serde_json::from_value(issue).unwrap();
        catalog.apply_success(devices, vec![issue]).unwrap();
        catalog.apply_error("peripherals.monitor_failed", "events stopped");
        let expected = "\
Peripherals  degraded  revision 3  scan 2  updated <time>
  Showing last-good records for a provider that could not be refreshed.

  TYPE     ID                                   PROVIDER                   NAME / MODES
  camera   a                                    p                          imx477 5-001a  (2 modes, 1 supported)
  camera   b                                    p                          evil?]0;owned?cam
  ! p io.open: gone?[2J
  ! peripherals.monitor_failed: events stopped
";
        assert_eq!(text(&catalog), expected);
    }

    /// The catalog file outlives the daemon, so only a running daemon whose
    /// health reports a peripheral summary vouches for it. The probe is one
    /// complete health request, so the daemon logs nothing.
    #[test]
    fn only_a_daemon_with_peripheral_discovery_vouches_for_the_catalog() {
        assert!(!daemon_serves_catalog(Path::new("/nonexistent/api.sock")));
        let dir = TempDir::new();
        let path = dir.path().join("api.sock");
        for (body, live) in [
            (r#"{"status":"ok","peripherals":{"ready":true}}"#, true),
            (r#"{"status":"ok","peripherals":null}"#, false), // --no-peripherals
            (r#"{"status":"ok"}"#, false),
            ("", false),
        ] {
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 256];
                let length = stream.read(&mut request).unwrap();
                let length_header = format!("Content-Length: {}", body.len());
                let response = format!("HTTP/1.1 200 OK\r\n{length_header}\r\n\r\n{body}");
                stream.write_all(response.as_bytes()).unwrap();
                String::from_utf8_lossy(&request[..length]).into_owned()
            });
            assert_eq!(daemon_serves_catalog(&path), live, "health body {body:?}");
            let request = server.join().unwrap();
            assert!(request.starts_with("GET /v1/health HTTP/1.1\r\n"));
        }
        let stale = daemon_serves_catalog(&path);
        assert!(!stale, "a stale socket file is not a running daemon");
    }
}
