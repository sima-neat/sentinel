use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

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

pub fn spawn(
    socket_path: &Path,
    cache_path: &Path,
    runs_dir: &Path,
    stopped: Arc<AtomicBool>,
) -> Result<JoinHandle<()>> {
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
    listener.set_nonblocking(true)?;

    let socket_path = socket_path.to_path_buf();
    let cache_path = cache_path.to_path_buf();
    let runs_dir = runs_dir.to_path_buf();
    Ok(thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    if let Err(error) = serve(&mut stream, &cache_path, &runs_dir) {
                        eprintln!("Sentinel API request failed: {error:#}");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => eprintln!("Sentinel API accept failed: {error}"),
            }
        }
        drop(listener);
        let _ = fs::remove_file(socket_path);
    }))
}

fn serve(stream: &mut UnixStream, cache_path: &Path, runs_dir: &Path) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let response =
        match read_request(stream).and_then(|request| route(request, cache_path, runs_dir)) {
            Ok(value) => response(200, value),
            Err(error) => response(error.status, json!({"error": error.message})),
        };
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
) -> std::result::Result<Value, ApiError> {
    let cache_path = cache_path
        .to_str()
        .ok_or_else(|| internal("cache path is not UTF-8"))?;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/health") => {
            let payload = cache::read_cache(cache_path).map_err(internal)?;
            let active = runs::active(runs_dir).map_err(internal)?;
            Ok(json!({
                "schema": 1,
                "version": payload.version,
                "updated_at": payload.updated_at,
                "latest_sample_at": payload.latest.map(|sample| sample.timestamp),
                "metric_count": payload.metrics.len(),
                "cached_samples": payload.samples.len(),
                "active_trace": active.map(|run| run.metadata),
                "errors": payload.errors,
            }))
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
            let active = runs::active(runs_dir).map_err(internal)?;
            Ok(json!({
                "schema": 1,
                "trace": active.as_ref().map(|run| &run.metadata),
                "summary": active.as_ref().map(runs::summary),
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
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
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
        let handle = spawn(&socket_path, &cache_path, &runs_dir, stopped.clone()).unwrap();

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
