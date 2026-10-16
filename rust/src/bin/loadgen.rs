#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use reqwest::{Client, Method, redirect::Policy};
use serde::Serialize;
use tokio::task::JoinSet;

const HISTOGRAM_RATIO: f64 = 1.01;
const MAX_BUCKET: usize = 4095;
const TOKEN: &str = "Bearer bench-token";

#[derive(Clone, Debug)]
struct Workload {
    name: &'static str,
    path: &'static str,
    method: Method,
    request_body: Option<&'static str>,
    status: u16,
    content_type: &'static str,
    response_body: &'static str,
}

fn workload(name: &str) -> Result<Workload, String> {
    let (path, method, request_body, status, content_type, response_body) = match name {
        "static" => (
            "/static",
            Method::GET,
            None,
            200,
            "application/json",
            r#"{"ok":true}"#,
        ),
        "numeric-path" => (
            "/items/42",
            Method::GET,
            None,
            200,
            "application/json",
            r#"{"id":42}"#,
        ),
        "nested-query" => (
            "/search?filter.active=true&values.0=1&values.1=2",
            Method::GET,
            None,
            200,
            "application/json",
            r#"{"filter":{"active":true},"values":[1,2]}"#,
        ),
        "validated-post" => (
            "/items",
            Method::POST,
            Some(r#"{"name":"sample","tags":["a","b"]}"#),
            201,
            "application/json",
            r#"{"name":"sample","count":2}"#,
        ),
        "html" => (
            "/html/42",
            Method::GET,
            None,
            200,
            "text/html; charset=utf-8",
            "<main><h1>Item 42</h1><p>Switchboard</p></main>",
        ),
        _ => return Err(format!("unknown workload: {name}")),
    };
    let name = match name {
        "static" => "static",
        "numeric-path" => "numeric-path",
        "nested-query" => "nested-query",
        "validated-post" => "validated-post",
        "html" => "html",
        _ => unreachable!(),
    };
    Ok(Workload {
        name,
        path,
        method,
        request_body,
        status,
        content_type,
        response_body,
    })
}

#[derive(Debug)]
struct Config {
    base_url: String,
    workload: Workload,
    duration_ms: u64,
    concurrency: usize,
    timeout_ms: u64,
}

fn number(value: &str, name: &str, max: u64) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("invalid {name}: {value}"))?;
    if parsed == 0 || parsed > max {
        return Err(format!("{name} must be in 1..={max}"));
    }
    Ok(parsed)
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut values = BTreeMap::new();
    let mut args = args.into_iter();
    while let Some(key) = args.next() {
        if !matches!(
            key.as_str(),
            "--base-url" | "--workload" | "--duration-ms" | "--concurrency" | "--timeout-ms"
        ) {
            return Err(format!("unknown argument: {key}"));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {key}"))?;
        if values.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate argument: {key}"));
        }
    }
    let base_url = values.get("--base-url").ok_or("--base-url required")?;
    let url = url::Url::parse(base_url).map_err(|error| format!("invalid base URL: {error}"))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(
            "base URL must be http://127.0.0.1:<port> with no path, credentials, query or fragment"
                .into(),
        );
    }
    let workload = workload(values.get("--workload").ok_or("--workload required")?)?;
    let duration_ms = number(
        values
            .get("--duration-ms")
            .map(String::as_str)
            .unwrap_or("1000"),
        "duration-ms",
        300_000,
    )?;
    let concurrency = number(
        values
            .get("--concurrency")
            .map(String::as_str)
            .unwrap_or("16"),
        "concurrency",
        1024,
    )? as usize;
    let timeout_ms = number(
        values
            .get("--timeout-ms")
            .map(String::as_str)
            .unwrap_or("5000"),
        "timeout-ms",
        60_000,
    )?;
    Ok(Config {
        base_url: base_url.trim_end_matches('/').to_owned(),
        workload,
        duration_ms,
        concurrency,
        timeout_ms,
    })
}

#[derive(Default)]
struct Measurements {
    completed: u64,
    transport_errors: u64,
    mismatches: u64,
    buckets: BTreeMap<usize, u64>,
    errors: Vec<String>,
}

impl Measurements {
    fn record_latency(&mut self, elapsed: Duration) {
        let nanos = elapsed.as_nanos().max(1) as f64;
        let bucket = ((nanos.ln() / HISTOGRAM_RATIO.ln()).ceil() as usize).min(MAX_BUCKET);
        *self.buckets.entry(bucket).or_default() += 1;
    }

    fn merge(&mut self, other: Self) {
        self.completed += other.completed;
        self.transport_errors += other.transport_errors;
        self.mismatches += other.mismatches;
        for (bucket, count) in other.buckets {
            *self.buckets.entry(bucket).or_default() += count;
        }
        for error in other.errors {
            if self.errors.len() < 8 {
                self.errors.push(error);
            }
        }
    }

    fn percentile_ms(&self, percentile: f64) -> f64 {
        let rank = (self.completed as f64 * percentile).ceil() as u64;
        let mut seen = 0;
        for (bucket, count) in &self.buckets {
            seen += count;
            if seen >= rank {
                return HISTOGRAM_RATIO.powi(*bucket as i32) / 1_000_000.0;
            }
        }
        0.0
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Latency {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    histogram_relative_error: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    workload: String,
    duration_ms: u64,
    concurrency: usize,
    timeout_ms: u64,
    elapsed_seconds: f64,
    completed: u64,
    transport_errors: u64,
    mismatches: u64,
    requests_per_second: f64,
    latency: Latency,
    errors: Vec<String>,
}

async fn request(client: &Client, url: &str, workload: &Workload) -> Result<(), (bool, String)> {
    let mut request = client
        .request(workload.method.clone(), url)
        .header("authorization", TOKEN)
        .header("accept", workload.content_type);
    if let Some(body) = workload.request_body {
        request = request
            .header("content-type", "application/json")
            .body(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| (false, error.to_string()))?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    // A bounded body keeps an invalid streaming response from exhausting client memory.
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| (false, error.to_string()))?
    {
        if body.len() + chunk.len() > 65_536 {
            return Err((true, "response body exceeds 65536 bytes".into()));
        }
        body.extend_from_slice(&chunk);
    }
    if status != workload.status
        || content_type != workload.content_type
        || body != workload.response_body.as_bytes()
    {
        return Err((
            true,
            format!(
                "{}: status={status}, content-type={content_type:?}, body={:?}",
                workload.name,
                String::from_utf8_lossy(&body)
            ),
        ));
    }
    Ok(())
}

async fn run(config: Config) -> Result<Report, String> {
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .http1_only()
        .pool_max_idle_per_host(config.concurrency)
        .timeout(Duration::from_millis(config.timeout_ms))
        .build()
        .map_err(|error| error.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(tokio::sync::Barrier::new(config.concurrency + 1));
    let start = Arc::new(std::sync::OnceLock::<Instant>::new());
    let mut tasks = JoinSet::new();
    for _ in 0..config.concurrency {
        let client = client.clone();
        let stop = stop.clone();
        let barrier = barrier.clone();
        let start = start.clone();
        let workload = config.workload.clone();
        let url = format!("{}{}", config.base_url, workload.path);
        let duration = Duration::from_millis(config.duration_ms);
        tasks.spawn(async move {
            let mut measurements = Measurements::default();
            barrier.wait().await;
            let start = *start.get().expect("start set before barrier release");
            while start.elapsed() < duration && !stop.load(Ordering::Relaxed) {
                let request_start = Instant::now();
                match request(&client, &url, &workload).await {
                    Ok(()) => {
                        measurements.completed += 1;
                        measurements.record_latency(request_start.elapsed());
                    }
                    Err((mismatch, error)) => {
                        if mismatch {
                            measurements.mismatches += 1;
                        } else {
                            measurements.transport_errors += 1;
                        }
                        measurements.errors.push(error);
                        stop.store(true, Ordering::Relaxed);
                    }
                }
            }
            measurements
        });
    }
    let began = Instant::now();
    start.set(began).map_err(|_| "start already initialized")?;
    barrier.wait().await;
    let mut measurements = Measurements::default();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(result) => measurements.merge(result),
            Err(error) => {
                stop.store(true, Ordering::Relaxed);
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                return Err(format!("load worker failed: {error}"));
            }
        }
    }
    let elapsed_seconds = began.elapsed().as_secs_f64();
    let latency = Latency {
        p50_ms: measurements.percentile_ms(0.50),
        p95_ms: measurements.percentile_ms(0.95),
        p99_ms: measurements.percentile_ms(0.99),
        histogram_relative_error: HISTOGRAM_RATIO - 1.0,
    };
    Ok(Report {
        workload: config.workload.name.to_owned(),
        duration_ms: config.duration_ms,
        concurrency: config.concurrency,
        timeout_ms: config.timeout_ms,
        elapsed_seconds,
        completed: measurements.completed,
        transport_errors: measurements.transport_errors,
        mismatches: measurements.mismatches,
        requests_per_second: measurements.completed as f64 / elapsed_seconds,
        latency,
        errors: measurements.errors,
    })
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let result = match parse_args(std::env::args().skip(1)) {
        Ok(config) => run(config).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(report) => {
            let success =
                report.completed > 0 && report.transport_errors == 0 && report.mismatches == 0;
            match serde_json::to_string(&report) {
                Ok(json) => println!("{json}"),
                Err(error) => {
                    eprintln!("{error}");
                    return std::process::ExitCode::FAILURE;
                }
            }
            if success {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> Vec<String> {
        [
            vec![
                "--base-url",
                "http://127.0.0.1:12345",
                "--workload",
                "static",
            ],
            extra.to_vec(),
        ]
        .concat()
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn validates_arguments_and_local_origin() {
        assert_eq!(parse_args(args(&[])).unwrap().concurrency, 16);
        for extra in [
            vec!["--concurrency", "0"],
            vec!["--duration-ms", "300001"],
            vec!["--what", "x"],
            vec!["--workload", "html"],
        ] {
            assert!(parse_args(args(&extra)).is_err());
        }
        assert!(
            parse_args(
                [
                    "--base-url",
                    "http://example.com:80",
                    "--workload",
                    "static"
                ]
                .map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn latency_percentiles_are_bounded_upper_estimates() {
        let mut measurements = Measurements::default();
        for micros in 1..=100 {
            measurements.completed += 1;
            measurements.record_latency(Duration::from_micros(micros));
        }
        for (quantile, expected) in [(0.50, 0.050), (0.95, 0.095), (0.99, 0.099)] {
            let actual = measurements.percentile_ms(quantile);
            assert!(
                actual >= expected && actual <= expected * HISTOGRAM_RATIO,
                "{actual} vs {expected}"
            );
        }
    }

    #[tokio::test]
    async fn detects_transport_errors_without_retrying() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut config = parse_args(args(&["--duration-ms", "20", "--concurrency", "1"])).unwrap();
        config.base_url = format!("http://127.0.0.1:{port}");
        let report = run(config).await.unwrap();
        assert_eq!(report.transport_errors, 1);
        assert_eq!(report.completed, 0);
        assert_eq!(report.mismatches, 0);
    }

    #[tokio::test]
    async fn verifies_bytes_status_and_content_type() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, content_type, body) in [
            (200, "application/json", r#"{"ok":true}"#),
            (200, "application/json", r#"{"ok":false}"#),
            (201, "application/json", r#"{"ok":true}"#),
            (200, "text/plain", r#"{"ok":true}"#),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                let mut request = Vec::new();
                loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "peer closed before request headers");
                    request.extend_from_slice(&buffer[..count]);
                    assert!(
                        request.len() <= 4096,
                        "unexpected oversized fixture request"
                    );
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let client = Client::builder()
                .no_proxy()
                .retry(reqwest::retry::never())
                .build()
                .unwrap();
            let result = request(
                &client,
                &format!("http://127.0.0.1:{port}/static"),
                &workload("static").unwrap(),
            )
            .await;
            assert_eq!(
                result.is_ok(),
                status == 200 && content_type == "application/json" && body == r#"{"ok":true}"#
            );
            server.await.unwrap();
        }
    }
}
