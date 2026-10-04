use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::de::IgnoredAny;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::peripherals::service::Control;
use crate::{cache, runs};

pub const DEFAULT_API_SOCKET: &str = "/run/simaai-sentinel/api.sock";
const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    query: String,
    body: Vec<u8>,
}

#[derive(Debug)]
struct ApiError {
    status: u16,
    message: String,
}

#[derive(Deserialize)]
struct StartTrace {
    name: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Where the API finds the peripheral catalog and how it asks for a rescan.
#[derive(Clone)]
pub struct PeripheralsApi {
    pub catalog_path: PathBuf,
    pub control: Option<Arc<Control>>,
}

pub struct ApiServer {
    thread: JoinHandle<()>,
    socket_path: std::path::PathBuf,
}

impl ApiServer {
    pub fn join(self) -> thread::Result<()> {
        // Wake a listener blocked in accept after the daemon sets its shared
        // stop flag. The server checks that flag before reading this stream.
        let _ = UnixStream::connect(&self.socket_path);
        self.thread.join()
    }
}

pub fn spawn(
    socket_path: &Path,
    cache_path: &Path,
    runs_dir: &Path,
    peripherals: Option<PeripheralsApi>,
    stopped: Arc<AtomicBool>,
) -> Result<ApiServer> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create API socket directory {}", parent.display()))?;
    }
    if socket_path.exists() {
        fs::remove_file(socket_path)
            .with_context(|| format!("remove stale API socket {}", socket_path.display()))?;
    }
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("bind Sentinel API socket {}", socket_path.display()))?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o666))
        .with_context(|| format!("set API socket permissions {}", socket_path.display()))?;
    let socket_path = socket_path.to_path_buf();
    let wake_path = socket_path.clone();
    let cache_path = cache_path.to_path_buf();
    let runs_dir = runs_dir.to_path_buf();
    let thread = thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok(_) if stopped.load(Ordering::Relaxed) => break,
                Ok((mut stream, _)) => {
                    if let Err(error) =
                        serve(&mut stream, &cache_path, &runs_dir, peripherals.as_ref())
                    {
                        eprintln!("Sentinel API request failed: {error:#}");
                    }
                }
                Err(error) => eprintln!("Sentinel API accept failed: {error}"),
            }
        }
        drop(listener);
        let _ = fs::remove_file(socket_path);
    });
    Ok(ApiServer {
        thread,
        socket_path: wake_path,
    })
}

fn serve(
    stream: &mut UnixStream,
    cache_path: &Path,
    runs_dir: &Path,
    peripherals: Option<&PeripheralsApi>,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let reply = read_request(stream).and_then(|request| {
        if request.method == "GET" && request.path == "/v1/peripherals" {
            peripheral_catalog(&request, peripherals)
        } else {
            route(request, cache_path, runs_dir, peripherals).map(|value| response(200, value))
        }
    });
    let response =
        reply.unwrap_or_else(|error| response(error.status, json!({"error": error.message})));
    stream.write_all(&response)?;
    Ok(())
}

fn read_request(stream: &mut UnixStream) -> std::result::Result<Request, ApiError> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).map_err(internal)?;
        if count == 0 {
            return Err(bad_request("connection closed before request was complete"));
        }
        data.extend_from_slice(&buffer[..count]);
        if data.len() > MAX_REQUEST_BYTES {
            return Err(ApiError {
                status: 413,
                message: "request exceeds 64 KiB".into(),
            });
        }
        if let Some(position) = find_bytes(&data, b"\r\n\r\n") {
            break position + 4;
        }
    };
    let header =
        std::str::from_utf8(&data[..header_end]).map_err(|_| bad_request("invalid HTTP header"))?;
    let mut lines = header.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| bad_request("missing request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or("").to_string();
    let target = request_parts.next().unwrap_or("").to_string();
    if method.is_empty() || target.is_empty() {
        return Err(bad_request("invalid request line"));
    }
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>())
        .transpose()
        .map_err(|_| bad_request("invalid Content-Length"))?
        .unwrap_or(0);
    if header_end + content_length > MAX_REQUEST_BYTES {
        return Err(ApiError {
            status: 413,
            message: "request exceeds 64 KiB".into(),
        });
    }
    while data.len() < header_end + content_length {
        let count = stream.read(&mut buffer).map_err(internal)?;
        if count == 0 {
            return Err(bad_request("request body is incomplete"));
        }
        data.extend_from_slice(&buffer[..count]);
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    Ok(Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        body: data[header_end..header_end + content_length].to_vec(),
    })
}

fn route(
    request: Request,
    cache_path: &Path,
    runs_dir: &Path,
    peripherals: Option<&PeripheralsApi>,
) -> std::result::Result<Value, ApiError> {
    let cache_path = cache_path
        .to_str()
        .ok_or_else(|| internal("cache path is not UTF-8"))?;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/health") => {
            let payload = cache::read_cache(cache_path).map_err(internal)?;
            let active = runs::active_metadata(runs_dir).map_err(internal)?;
            Ok(json!({
                "schema": 1,
                "version": payload.version,
                "updated_at": payload.updated_at,
                "latest_sample_at": payload.latest.map(|sample| sample.timestamp),
                "metric_count": payload.metrics.len(),
                "cached_samples": payload.samples.len(),
                "active_trace": active.map(|run| run.metadata),
                "errors": payload.errors,
                "peripherals": serving(peripherals).ok().and_then(peripheral_summary),
            }))
        }
        ("POST", "/v1/peripherals/refresh") => {
            let control = peripherals
                .and_then(|peripherals| peripherals.control.as_ref())
                .ok_or_else(peripherals_disabled)?;
            let target = control.request_refresh().ok_or_else(peripherals_stopped)?;
            Ok(json!({"accepted": true, "target_scan_sequence": target}))
        }
        ("GET", "/v1/cache") => {
            serde_json::to_value(cache::read_cache(cache_path).map_err(internal)?).map_err(internal)
        }
        ("GET", "/v1/metrics") => {
            let payload = cache::read_cache(cache_path).map_err(internal)?;
            Ok(json!({"schema": 1, "metrics": payload.metrics}))
        }
        ("GET", "/v1/samples/latest") => {
            let payload = cache::read_cache(cache_path).map_err(internal)?;
            Ok(json!({
                "schema": 1,
                "version": payload.version,
                "updated_at": payload.updated_at,
                "sample": payload.latest,
            }))
        }
        ("GET", "/v1/traces/active") => {
            let active = runs::active_status(runs_dir).map_err(internal)?;
            Ok(json!({
                "schema": 1,
                "trace": active.as_ref().map(|(metadata, _)| metadata),
                "summary": active.as_ref().map(|(_, summary)| summary),
            }))
        }
        ("POST", "/v1/traces") => {
            let input: StartTrace = serde_json::from_slice(&request.body)
                .map_err(|error| bad_request(format!("invalid trace request: {error}")))?;
            let payload = cache::read_cache(cache_path).map_err(internal)?;
            let run = runs::start(runs_dir, &payload, &input.name, input.note, input.tags)
                .map_err(conflict)?;
            Ok(json!({"schema": 1, "trace": run.metadata}))
        }
        ("POST", "/v1/traces/stop") => {
            let run = runs::stop(runs_dir).map_err(conflict)?;
            let summary = runs::summary(&run);
            Ok(json!({"schema": 1, "trace": run.metadata, "summary": summary}))
        }
        ("GET", "/v1/runs") => Ok(json!({
            "schema": 1,
            "runs": runs::list(runs_dir).map_err(internal)?,
        })),
        ("GET", "/v1/compare") => {
            let selectors = query_values(&request.query, "runs");
            if selectors.len() < 2 {
                return Err(bad_request("compare requires at least two runs parameters"));
            }
            let selected = runs::load_many(runs_dir, &selectors).map_err(not_found)?;
            let metadata: Vec<_> = selected.iter().map(|run| run.metadata.clone()).collect();
            let include_raw = query_values(&request.query, "raw")
                .first()
                .is_some_and(|value| value == "1" || value == "true");
            let mut document =
                serde_json::to_value(runs::export_json(selected)).map_err(internal)?;
            if !include_raw {
                document["runs"] = serde_json::to_value(metadata).map_err(internal)?;
            }
            Ok(document)
        }
        ("GET", path) if path.starts_with("/v1/runs/") => {
            let selector = percent_decode(&path[9..])?;
            serde_json::to_value(runs::load(runs_dir, &selector).map_err(not_found)?)
                .map_err(internal)
        }
        _ => Err(ApiError {
            status: 404,
            message: "unknown Sentinel API endpoint".into(),
        }),
    }
}

/// The fields the API needs from the catalog. Deserializing only these (and
/// counting, not building, the arrays) keeps each request cheap.
#[derive(Deserialize)]
struct CatalogHeader {
    schema_version: u32,
    instance_id: String,
    state: String,
    ready: bool,
    stale: bool,
    revision: u64,
    scan_sequence: u64,
    devices: Vec<IgnoredAny>,
    issues: Vec<IgnoredAny>,
}

/// `GET /v1/peripherals`: the catalog file's bytes exactly as the
/// peripherals thread wrote them, or a short reply when the client is current.
fn peripheral_catalog(
    request: &Request,
    peripherals: Option<&PeripheralsApi>,
) -> std::result::Result<Vec<u8>, ApiError> {
    let peripherals = serving(peripherals)?;
    let since = query_values(&request.query, "since_revision")
        .first()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| bad_request("since_revision must be a non-negative integer"))
        })
        .transpose()?;
    let body = fs::read(&peripherals.catalog_path).map_err(|error| {
        unavailable(format!(
            "peripheral catalog unavailable: {}: {error}",
            peripherals.catalog_path.display()
        ))
    })?;
    if let Some(since) = since {
        // Revisions restart with every daemon, so a revision alone cannot
        // prove the client is current.
        let instance = query_values(&request.query, "instance_id")
            .into_iter()
            .next()
            .ok_or_else(|| bad_request("since_revision requires instance_id"))?;
        let header: CatalogHeader = serde_json::from_slice(&body)
            .map_err(|error| unavailable(format!("peripheral catalog unreadable: {error}")))?;
        if instance == header.instance_id && since == header.revision && header.ready {
            return Ok(response(
                200,
                json!({
                    "schema_version": header.schema_version,
                    "instance_id": header.instance_id,
                    "revision": header.revision,
                    "scan_sequence": header.scan_sequence,
                    "unchanged": true,
                }),
            ));
        }
    }
    Ok(raw_response(200, body))
}

fn peripheral_summary(peripherals: &PeripheralsApi) -> Option<Value> {
    let body = fs::read(&peripherals.catalog_path).ok()?;
    let header: CatalogHeader = serde_json::from_slice(&body).ok()?;
    Some(json!({
        "instance_id": header.instance_id,
        "state": header.state,
        "ready": header.ready,
        "stale": header.stale,
        "revision": header.revision,
        "scan_sequence": header.scan_sequence,
        "device_count": header.devices.len(),
        "issue_count": header.issues.len(),
    }))
}

/// The peripherals API while its catalog is current. After the discovery
/// thread exits (a panic included), the file keeps its last contents, so it
/// is refused like a refresh instead of being served as live.
fn serving(peripherals: Option<&PeripheralsApi>) -> std::result::Result<&PeripheralsApi, ApiError> {
    let peripherals = peripherals.ok_or_else(peripherals_disabled)?;
    match &peripherals.control {
        Some(control) if !control.is_alive() => Err(peripherals_stopped()),
        _ => Ok(peripherals),
    }
}

fn peripherals_stopped() -> ApiError {
    unavailable("peripheral discovery has stopped; see `journalctl -u simaai-sentinel`")
}

fn peripherals_disabled() -> ApiError {
    unavailable(
        "peripheral discovery is not running in this Sentinel daemon; \
see `journalctl -u simaai-sentinel`",
    )
}

fn query_values(query: &str, key: &str) -> Vec<String> {
    query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .filter(|(name, _)| *name == key)
        .filter_map(|(_, value)| percent_decode(value).ok())
        .flat_map(|value| value.split(',').map(str::to_string).collect::<Vec<_>>())
        .filter(|value| !value.is_empty())
        .collect()
}

fn percent_decode(value: &str) -> std::result::Result<String, ApiError> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                    .map_err(|_| bad_request("invalid URL encoding"))?;
                output.push(
                    u8::from_str_radix(hex, 16).map_err(|_| bad_request("invalid URL encoding"))?,
                );
                index += 3;
            }
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).map_err(|_| bad_request("URL is not UTF-8"))
}

fn response(status: u16, body: Value) -> Vec<u8> {
    let body = serde_json::to_vec_pretty(&body)
        .unwrap_or_else(|_| b"{\"error\":\"serialization failed\"}".to_vec());
    raw_response(status, body)
}

fn raw_response(status: u16, body: Vec<u8>) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    [header.as_bytes(), &body].concat()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError {
        status: 400,
        message: message.into(),
    }
}

fn conflict(error: impl std::fmt::Display) -> ApiError {
    ApiError {
        status: 409,
        message: error.to_string(),
    }
}

fn not_found(error: impl std::fmt::Display) -> ApiError {
    ApiError {
        status: 404,
        message: error.to_string(),
    }
}

fn unavailable(message: impl Into<String>) -> ApiError {
    ApiError {
        status: 503,
        message: message.into(),
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError {
        status: 500,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::model::{CachePayload, Sample};

    #[test]
    fn api_controls_trace_and_returns_live_data() {
        let root = std::env::temp_dir().join(format!(
            "sentinel-api-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let cache_path = root.join("cache.json");
        let runs_dir = root.join("runs");
        let socket_path = root.join("api.sock");
        let sample = Sample {
            timestamp: Utc::now(),
            values: BTreeMap::from([("power_current_watts".into(), Some(8.5))]),
        };
        let payload = CachePayload {
            schema: 1,
            version: "test".into(),
            updated_at: Utc::now(),
            metrics: Vec::new(),
            latest: Some(sample.clone()),
            samples: vec![sample],
            processes: Vec::new(),
            power: None,
            errors: Vec::new(),
        };
        cache::write_cache(cache_path.to_str().unwrap(), &payload).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let handle = spawn(&socket_path, &cache_path, &runs_dir, None, stopped.clone()).unwrap();

        let latest = request(
            &socket_path,
            "GET /v1/samples/latest HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(latest.contains("power_current_watts"));
        let started = request(
            &socket_path,
            "POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Length: 31\r\n\r\n{\"name\":\"agent-test\",\"tags\":[]}",
        );
        assert!(started.contains("agent-test"), "{started}");
        let stopped_response = request(
            &socket_path,
            "POST /v1/traces/stop HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
        );
        assert!(stopped_response.contains("agent-test"));
        request(
            &socket_path,
            "POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Length: 33\r\n\r\n{\"name\":\"agent-test-2\",\"tags\":[]}",
        );
        request(
            &socket_path,
            "POST /v1/traces/stop HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
        );
        let compact = response_json(&request(
            &socket_path,
            "GET /v1/compare?runs=agent-test,agent-test-2 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        ));
        assert!(compact["runs"][0].get("samples").is_none());
        let raw = response_json(&request(
            &socket_path,
            "GET /v1/compare?runs=agent-test,agent-test-2&raw=1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        ));
        assert!(raw["runs"][0]["samples"].is_array());

        stopped.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn peripheral_catalog_is_served_as_written() {
        let root = std::env::temp_dir().join(format!("sentinel-api-p-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let catalog_path = root.join("peripherals.json");
        let mut catalog = crate::peripherals::catalog::Catalog::new("instance-a", 8);
        let camera = crate::peripherals::scan::testing::record("p", "camera:x", json!({}));
        catalog.apply_success(vec![camera], vec![]).unwrap();
        crate::peripherals::service::publish(&catalog_path, &catalog.document()).unwrap();
        let (socket, stopped) = (root.join("api.sock"), Arc::new(AtomicBool::new(false)));
        let peripherals = Some(PeripheralsApi {
            catalog_path: catalog_path.clone(),
            control: None,
        });
        let (cache, runs) = (root.join("cache.json"), root.join("runs"));
        let handle = spawn(&socket, &cache, &runs, peripherals, stopped.clone()).unwrap();
        let get = |query: &str| {
            let get = format!("GET /v1/peripherals{query} HTTP/1.1\r\nHost: localhost\r\n\r\n");
            request(&socket, &get)
        };

        let full = get("");
        let body = full.split_once("\r\n\r\n").unwrap().1;
        assert_eq!(body.as_bytes(), fs::read(&catalog_path).unwrap());
        let current = response_json(&get("?since_revision=1&instance_id=instance-a"));
        assert_eq!(current["unchanged"], true);
        let other_instance = response_json(&get("?since_revision=1&instance_id=instance-b"));
        assert_eq!(other_instance["devices"][0]["id"], "camera:x");
        for query in ["?since_revision=x", "?since_revision=1"] {
            assert!(get(query).starts_with("HTTP/1.1 400"), "{query}");
        }
        let refresh = "POST /v1/peripherals/refresh HTTP/1.1\r\nContent-Length: 0\r\n\r\n";
        let refresh = request(&socket, refresh);
        assert!(refresh.starts_with("HTTP/1.1 503"), "no control");

        stopped.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// Once the discovery thread has exited (a panic marks it dead the same
    /// way), its last catalog is no longer current: the catalog route answers
    /// 503 like refresh, and health drops the summary the CLI trusts.
    #[test]
    fn catalog_is_refused_once_discovery_thread_exits() {
        use crate::peripherals::scan::testing::{record, Fake};
        use crate::peripherals::service;
        let root = std::env::temp_dir().join(format!("sentinel-api-d-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let catalog_path = root.join("peripherals.json");
        let config = service::Config {
            catalog_path: catalog_path.clone(),
            support_rules_path: root.join("neat-core.json"),
            instance_id: "instance-a".into(),
            debounce: Duration::from_millis(10),
            listen_for_uevents: false,
        };
        let camera = record("p", "camera:x", json!({}));
        let provider = Fake("p", move || Ok(vec![camera.clone()]));
        let thread = service::spawn(config, vec![Box::new(provider)]).unwrap();
        let peripherals = Some(PeripheralsApi {
            catalog_path,
            control: Some(thread.control()),
        });
        let (socket, stopped) = (root.join("api.sock"), Arc::new(AtomicBool::new(false)));
        let (cache, runs) = (root.join("cache.json"), root.join("runs"));
        let handle = spawn(&socket, &cache, &runs, peripherals, stopped.clone()).unwrap();
        let payload = CachePayload {
            schema: 1,
            version: "test".into(),
            updated_at: Utc::now(),
            metrics: Vec::new(),
            latest: None,
            samples: Vec::new(),
            processes: Vec::new(),
            power: None,
            errors: Vec::new(),
        };
        cache::write_cache(cache.to_str().unwrap(), &payload).unwrap();
        let get = "GET /v1/peripherals HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let health = "GET /v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert!(request(&socket, get).starts_with("HTTP/1.1 200"));
        let summary = &response_json(&request(&socket, health))["peripherals"];
        assert_eq!(summary["instance_id"], "instance-a");

        thread.stop();
        let stopped_text = "peripheral discovery has stopped; see `journalctl -u simaai-sentinel`";
        for query in ["", "?since_revision=1&instance_id=instance-a"] {
            let get = format!("GET /v1/peripherals{query} HTTP/1.1\r\nHost: localhost\r\n\r\n");
            let reply = request(&socket, &get);
            assert!(reply.starts_with("HTTP/1.1 503"), "{query}: {reply}");
            assert_eq!(response_json(&reply)["error"], stopped_text);
        }
        let refresh = "POST /v1/peripherals/refresh HTTP/1.1\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            response_json(&request(&socket, refresh))["error"],
            stopped_text
        );
        assert!(response_json(&request(&socket, health))["peripherals"].is_null());

        stopped.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn request(socket_path: &Path, request: &str) -> String {
        let mut stream = UnixStream::connect(socket_path).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn response_json(response: &str) -> Value {
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }
}
