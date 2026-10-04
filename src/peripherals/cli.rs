use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde_json::Value;

use super::Catalog;

const REFRESH_TIMEOUT: Duration = Duration::from_secs(15);

/// `simaai-sentinel peripherals [--json] [--refresh]`
pub fn run(api_socket: &Path, args: &[String]) -> Result<()> {
    let (mut json, mut refresh) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--refresh" => refresh = true,
            other => bail!("unknown peripherals option '{other}'"),
        }
    }
    let catalog = if refresh {
        refreshed(api_socket)?
    } else {
        fetch(api_socket)?
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&catalog)?);
    } else {
        print!("{}", render(&catalog));
    }
    Ok(())
}

/// Request a rescan and wait for a catalog observed after the request.
fn refreshed(api_socket: &Path) -> Result<Catalog> {
    let requested = Utc::now();
    request(api_socket, "POST", "/v1/peripherals/refresh")?;
    let deadline = Instant::now() + REFRESH_TIMEOUT;
    loop {
        let catalog = fetch(api_socket)?;
        if catalog.observed_at.is_some_and(|at| at >= requested) {
            return Ok(catalog);
        }
        if Instant::now() >= deadline {
            bail!("refresh did not complete within 15 seconds");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn fetch(api_socket: &Path) -> Result<Catalog> {
    serde_json::from_value(request(api_socket, "GET", "/v1/peripherals")?)
        .context("parse peripheral catalog")
}

/// Send one request to the daemon; a non-2xx status becomes the error.
fn request(api_socket: &Path, method: &str, path: &str) -> Result<Value> {
    let mut stream = UnixStream::connect(api_socket).with_context(|| {
        format!(
            "connect to {}; check `systemctl status simaai-sentinel`",
            api_socket.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .context("invalid response from the Sentinel daemon")?;
    let value: Value = serde_json::from_str(body).context("parse Sentinel response")?;
    if !head.starts_with("HTTP/1.1 2") {
        bail!("{}", value["error"].as_str().unwrap_or(head));
    }
    Ok(value)
}

/// Device names and errors come from the hardware; replace control
/// characters, newlines included, so a device can neither send escape
/// sequences to the administrator's terminal nor forge rows.
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

fn render(catalog: &Catalog) -> String {
    let Some(observed_at) = catalog.observed_at else {
        return "Peripherals: the first scan has not completed.\n".into();
    };
    let mut out = format!(
        "Peripherals  revision {}  observed {}\n",
        catalog.revision,
        observed_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    if catalog.devices.is_empty() {
        out.push_str("  No peripherals found.\n");
    } else {
        out.push_str(&format!("\n  {:<12} {:<36} {}\n", "TYPE", "ID", "DETAILS"));
        for device in &catalog.devices {
            out.push_str(&format!(
                "  {:<12} {:<36} {}\n",
                device.kind(),
                printable(device.id()),
                printable(&device.describe())
            ));
        }
    }
    for error in &catalog.errors {
        out.push_str(&format!(
            "  ! {} {}: {}\n",
            printable(&error.provider),
            printable(&error.code),
            printable(&error.reason)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::device;
    use super::super::CatalogError;
    use super::*;

    #[test]
    fn render_lists_devices_and_escapes_hardware_text() {
        let mut catalog = Catalog {
            revision: 3,
            observed_at: None,
            devices: vec![device("test:\x1b[2Jcam")],
            errors: vec![CatalogError {
                provider: "test.scripted".into(),
                code: "io.permission_denied".into(),
                reason: "line\nforged row".into(),
            }],
        };
        assert_eq!(
            render(&catalog),
            "Peripherals: the first scan has not completed.\n"
        );
        catalog.observed_at = Some(Utc::now());
        let text = render(&catalog);
        assert!(text.contains("test:?[2Jcam"), "{text}");
        assert!(
            text.contains("! test.scripted io.permission_denied: line?forged row"),
            "{text}"
        );
        assert!(!text.contains('\x1b'));
    }
}
