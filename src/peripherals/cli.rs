use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::model::CatalogDocument;
use super::scan::validate;
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
    let mut document = fetch_catalog(api_socket).context(CATALOG_UNAVAILABLE)?;
    if refresh {
        let instance = document.instance_id.clone();
        let target = request_refresh(api_socket, catalog_path, &instance)?;
        document = wait_for_scan(api_socket, &instance, target)?;
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

/// Read the catalog through the daemon, which validates ownership, mode and
/// that the file is its latest published inode before returning the bytes.
fn fetch_catalog(api_socket: &Path) -> Result<CatalogDocument> {
    let response = exchange(
        api_socket,
        b"GET /v1/peripherals HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .context("invalid response from Sentinel daemon")?;
    if !head.starts_with("HTTP/1.1 200 ") {
        let value: Value = serde_json::from_str(body).context("parse catalog refusal")?;
        bail!(
            "{}",
            value["error"]
                .as_str()
                .unwrap_or("Sentinel daemon refused the peripheral catalog")
        );
    }
    serde_json::from_str(body).context("parse peripheral catalog response")
}

/// Ask the daemon for a scan, and return the `scan_sequence` that completes
/// it in `catalog_path`. The daemon's target counts the scans of its own
/// catalog, so it is refused unless that catalog is the one being read.
fn request_refresh(api_socket: &Path, catalog_path: &Path, instance: &str) -> Result<u64> {
    let response = exchange(
        api_socket,
        b"POST /v1/peripherals/refresh HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
    )?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_default();
    let value: Value = serde_json::from_str(body).context("parse refresh response")?;
    let target = value["target_scan_sequence"].as_u64().with_context(|| {
        format!(
            "refresh refused: {}",
            value["error"].as_str().unwrap_or("unexpected response")
        )
    })?;
    if value["instance_id"].as_str() != Some(instance) {
        bail!(
            "the Sentinel daemon does not publish {}: it was started with another \
--peripherals-file, or it restarted; pass the daemon's --peripherals-file or run the command again",
            catalog_path.display()
        );
    }
    Ok(target)
}

fn wait_for_scan(api_socket: &Path, instance: &str, target: u64) -> Result<CatalogDocument> {
    let start = Instant::now();
    loop {
        let document = match fetch_catalog(api_socket) {
            Ok(document) => document,
            Err(error) => bail!("refresh did not complete: {error}"),
        };
        if document.instance_id != instance {
            bail!("Sentinel restarted during the refresh; run the command again");
        }
        if document.scan_sequence >= target {
            return Ok(document);
        }
        if start.elapsed() >= Duration::from_secs(15) {
            bail!("refresh did not complete within 15 seconds");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Device names and errors come from the hardware; replace control
/// characters, newlines included, so a device can neither send escape
/// sequences to the administrator's terminal nor forge rows. Applied to each
/// field, so the renderer's own newlines stay.
fn printable(text: &str) -> String {
    text.chars()
        .map(|ch| {
            let mut escaped = ch.escape_debug();
            let prints_as_itself = escaped.next() == Some(ch) && escaped.next().is_none();
            if (!ch.is_ascii() && !prints_as_itself) || ch.is_control() {
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
        printable(&document.state),
        document.revision,
        document.scan_sequence,
        printable(document.last_attempt_at.as_deref().unwrap_or("never")),
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
            let name = printable(name);
            let summary = match modes {
                Some(modes) => format!("{name}  ({} modes, {supported} supported)", modes.len()),
                None => name,
            };
            out.push_str(&format!(
                "  {:<8} {:<36} {:<26} {}\n",
                printable(kind),
                printable(device["id"].as_str().unwrap_or("?")),
                printable(device["provider"].as_str().unwrap_or("?")),
                summary
            ));
        }
    }
    for issue in &document.issues {
        out.push_str(&format!(
            "  ! {} {}: {}\n",
            printable(&issue.provider),
            printable(&issue.code),
            printable(&issue.reason)
        ));
    }
    if let Some(error) = &document.error {
        out.push_str(&format!(
            "  ! {}: {}\n",
            printable(error["code"].as_str().unwrap_or("error")),
            printable(error["reason"].as_str().unwrap_or(""))
        ));
    }
    out
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

    /// Device names and errors come from the hardware, so control characters,
    /// newlines included, are replaced before they reach the terminal and
    /// cannot forge rows.
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
        let evil =
            json!({"model": "evil\u{1b}]0;owned\u{7}cam\n  ! forged row\u{202e}rtl\u{2066}"});
        let devices = vec![record("p", "a", imx477), record("p\n", "b\nc", evil)];
        let issue = json!({"provider": "p", "code": "io.open", "reason": "gone\u{1b}[2J\n",
                           "retained_last_good": true});
        let issue: Issue = serde_json::from_value(issue).unwrap();
        catalog.apply_success(devices, vec![issue]).unwrap();
        catalog.apply_error("peripherals.monitor_failed", "events\nstopped");
        let expected = "\
Peripherals  degraded  revision 3  scan 2  updated <time>
  Showing last-good records for a provider that could not be refreshed.

  TYPE     ID                                   PROVIDER                   NAME / MODES
  camera   a                                    p                          imx477 5-001a  (2 modes, 1 supported)
  camera   b?c                                  p?                         evil?]0;owned?cam?  ! forged row?rtl?
  ! p io.open: gone?[2J?
  ! peripherals.monitor_failed: events?stopped
";
        assert_eq!(text(&catalog), expected);
    }

    /// The CLI reads the daemon-validated endpoint, not the catalog path an
    /// unprivileged user may be able to replace.
    #[test]
    fn catalog_is_read_from_the_daemon_api() {
        let nonexistent = Path::new("/nonexistent/api.sock");
        assert!(fetch_catalog(nonexistent).is_err());
        let dir = TempDir::new();
        let path = dir.path().join("api.sock");
        let body = serde_json::to_string(&Catalog::new("i", 8).document()).unwrap();
        let server = answer_once(&path, "200 OK", &body);
        assert_eq!(fetch_catalog(&path).unwrap().instance_id, "i");
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /v1/peripherals HTTP/1.1\r\n"));

        let body = r#"{"error":"catalog publication was replaced"}"#;
        let server = answer_once(&path, "503 Service Unavailable", body);
        assert_eq!(
            fetch_catalog(&path).unwrap_err().to_string(),
            "catalog publication was replaced"
        );
        server.join().unwrap();
    }

    /// A refresh target counts the scans of the daemon's own catalog, so when
    /// the file being read is another one (another `--peripherals-file`, or an
    /// earlier daemon's), the CLI refuses instead of waiting for a scan that
    /// file will never record.
    #[test]
    fn refresh_targets_only_the_catalog_being_read() {
        let dir = TempDir::new();
        let socket = dir.path().join("api.sock");
        let file = Path::new("/run/other/peripherals.json");
        let refresh = |status, body| {
            let server = answer_once(&socket, status, body);
            let target = request_refresh(&socket, file, "i").map_err(|e| e.to_string());
            let request = server.join().unwrap();
            assert!(request.starts_with("POST /v1/peripherals/refresh HTTP/1.1\r\n"));
            target
        };
        let accepted = r#"{"accepted":true,"target_scan_sequence":5,"instance_id":"i"}"#;
        assert_eq!(refresh("200 OK", accepted), Ok(5));
        let other = "the Sentinel daemon does not publish /run/other/peripherals.json: it was \
started with another --peripherals-file, or it restarted; pass the daemon's --peripherals-file or \
run the command again";
        for body in [
            r#"{"accepted":true,"target_scan_sequence":5,"instance_id":"j"}"#,
            r#"{"accepted":true,"target_scan_sequence":5}"#,
        ] {
            assert_eq!(refresh("200 OK", body), Err(other.into()), "{body}");
        }
        let refused = r#"{"error":"peripheral discovery has stopped"}"#;
        let refused = refresh("503 Service Unavailable", refused);
        assert_eq!(
            refused.unwrap_err(),
            "refresh refused: peripheral discovery has stopped"
        );
    }

    /// While the daemon cannot write the catalog, the file stops advancing;
    /// the wait reports the daemon's reason within about a second instead of
    /// running out its 15 seconds.
    #[test]
    fn refresh_reports_a_catalog_the_daemon_cannot_write() {
        let dir = TempDir::new();
        let socket = dir.path().join("api.sock");
        let reason = "peripheral catalog could not be written: replace p.json: No space left \
on device (os error 28); see `journalctl -u simaai-sentinel`";
        let body = json!({ "error": reason }).to_string();
        let server = answer_once(&socket, "503 Service Unavailable", &body);
        let started = Instant::now();
        let error = wait_for_scan(&socket, "i", 2).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("refresh did not complete: {reason}")
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        let probe = "GET /v1/peripherals HTTP/1.1\r\n";
        assert!(server.join().unwrap().starts_with(probe));
    }

    /// Answer one request on a fresh socket at `path`; returns the request.
    fn answer_once(path: &Path, status: &str, body: &str) -> thread::JoinHandle<String> {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 256];
            let length = stream.read(&mut request).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request[..length]).into_owned()
        })
    }
}
