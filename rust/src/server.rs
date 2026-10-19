//! HTTP/TLS hosting with owned hot-reload sockets and log subscriptions.

use crate::logger::LogWatcher;
use crate::{Request, Response, Router, response};
use axum::body::{Body, HttpBody};
use futures_util::{SinkExt, StreamExt, future::BoxFuture};
use hyper::{body::Incoming, service::service_fn, upgrade::OnUpgrade};
use hyper_util::rt::TokioIo;
use std::{
    convert::Infallible,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    net::SocketAddr,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpListener,
    sync::{broadcast, mpsc, watch},
    task::{JoinHandle, JoinSet},
    time::{Sleep, sleep},
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{self, pki_types::PrivateKeyDer},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, handshake::derive_accept_key, protocol::Role},
};

#[derive(Clone)]
pub enum Https {
    Pem { cert: String, key: String },
    Generate { domain: String, cache_path: PathBuf },
}

impl Https {
    pub fn generate() -> Self {
        Self::Generate {
            domain: "localhost".into(),
            cache_path: ".genCert".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Redirect {
    pub port: u16,
    /// The reference redirects to 443 even when HTTPS listens on another port.
    pub https_port: u16,
}

impl Default for Redirect {
    fn default() -> Self {
        Self {
            port: 80,
            https_port: 443,
        }
    }
}

#[derive(Clone)]
pub struct ServerConfig {
    pub hostname: String,
    pub port: Option<u16>,
    pub https: Option<Https>,
    pub redirect: Option<Redirect>,
    pub idle_timeout: Duration,
    /// Total streamed request-body bytes. Zero permits only empty bodies.
    pub max_request_body_size: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            hostname: "0.0.0.0".into(),
            port: None,
            https: None,
            redirect: Some(Redirect::default()),
            idle_timeout: Duration::from_secs(10),
            max_request_body_size: 128 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AccessLogEntry {
    pub method: http::Method,
    pub url: url::Url,
    pub headers: http::HeaderMap,
}

pub type AccessLog = Arc<dyn Fn(Duration, &AccessLogEntry, &Response) + Send + Sync>;
pub type ReadLogs = Arc<dyn Fn() -> BoxFuture<'static, io::Result<String>> + Send + Sync>;
pub type WatchLogs =
    Arc<dyn Fn(Arc<dyn Fn() + Send + Sync>) -> io::Result<Box<dyn LogWatcher>> + Send + Sync>;

pub struct ServerOptions {
    pub access_log: Option<AccessLog>,
    pub read_logs: Option<ReadLogs>,
    pub watch_logs: Option<WatchLogs>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            access_log: Some(Arc::new(|duration, request, response| {
                tracing::info!(method = request.method.as_str(), url = %request.url,
                    status = response.status().as_u16(), elapsed_ms = duration.as_secs_f64() * 1000.0,
                    "request completed");
            })),
            read_logs: None,
            watch_logs: None,
        }
    }
}

#[derive(Debug)]
pub enum ServerError {
    Io(io::Error),
    Tls(String),
    Task(tokio::task::JoinError),
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "hosting I/O: {e}"),
            Self::Tls(e) => write!(f, "TLS: {e}"),
            Self::Task(e) => write!(f, "hosting task: {e}"),
        }
    }
}
impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Task(e) => Some(e),
            Self::Tls(_) => None,
        }
    }
}
impl From<io::Error> for ServerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

struct OwnedWatcher(Option<Box<dyn LogWatcher>>);
impl Drop for OwnedWatcher {
    fn drop(&mut self) {
        if let Some(watcher) = &mut self.0 {
            watcher.close();
        }
    }
}

/// Dropping initiates shutdown. Await `stop` to join cleanup and guarantee listener reuse.
pub struct ServerHandle {
    addr: SocketAddr,
    redirect_addr: Option<SocketAddr>,
    shutdown: watch::Sender<bool>,
    messages: broadcast::Sender<String>,
    task: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
    pub fn redirect_addr(&self) -> Option<SocketAddr> {
        self.redirect_addr
    }
    pub fn reload(&self) {
        let _ = self.messages.send("RELOAD".into());
    }
    pub async fn stop(&mut self) -> Result<(), ServerError> {
        let _ = self.shutdown.send(true);
        let result = if let Some(task) = &mut self.task {
            task.await.map_err(ServerError::Task)
        } else {
            Ok(())
        };
        self.task.take();
        result
    }
}
impl Drop for ServerHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// Hosts HTTP/TLS with automatic hot-reload and log WebSocket upgrades.
pub async fn serve(
    config: ServerConfig,
    router: Arc<Router>,
    options: ServerOptions,
) -> Result<ServerHandle, ServerError> {
    serve_with_websockets(config, router, options, true).await
}

/// Hosts HTTP/TLS without automatic WebSocket upgrades.
/// Upgrade requests pass through the router and normal access logging.
pub async fn serve_http_only(
    config: ServerConfig,
    router: Arc<Router>,
    options: ServerOptions,
) -> Result<ServerHandle, ServerError> {
    serve_with_websockets(config, router, options, false).await
}

async fn serve_with_websockets(
    config: ServerConfig,
    router: Arc<Router>,
    options: ServerOptions,
    websockets: bool,
) -> Result<ServerHandle, ServerError> {
    let tls = config.https.as_ref().map(tls_acceptor).transpose()?;
    let (messages, _) = broadcast::channel(64);
    let (changes, mut changed) = mpsc::channel(1);
    let watcher = if options.read_logs.is_some() {
        options
            .watch_logs
            .as_ref()
            .map(|watch| {
                watch(Arc::new(move || {
                    let _ = changes.try_send(());
                }))
            })
            .transpose()?
    } else {
        None
    };
    let watcher = OwnedWatcher(watcher);
    let port = config.port.unwrap_or(if tls.is_some() { 443 } else { 80 });
    // Bind everything before spawning: any startup failure drops both listeners and the watcher.
    let listener = TcpListener::bind((config.hostname.as_str(), port)).await?;
    let addr = listener.local_addr()?;
    let redirect = if tls.is_some() && port != 80 {
        if let Some(redirect) = &config.redirect {
            Some(TcpListener::bind((config.hostname.as_str(), redirect.port)).await?)
        } else {
            None
        }
    } else {
        None
    };
    let redirect_addr = redirect.as_ref().map(TcpListener::local_addr).transpose()?;
    let (shutdown, mut stopped) = watch::channel(false);
    let state = Arc::new(State {
        router,
        options,
        messages: messages.clone(),
        tls: tls.is_some(),
        max_request_body_size: config.max_request_body_size,
        websockets,
    });
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        let mut log_read: Option<BoxFuture<'static, io::Result<String>>> = None;
        loop {
            tokio::select! {
                _ = stopped.changed() => break,
                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = result { tracing::warn!(%error, "connection task failed"); }
                }
                Some(()) = changed.recv(), if log_read.is_none() => {
                    if let Some(read) = &state.options.read_logs { log_read = Some(read()); }
                }
                logs = async { log_read.as_mut().expect("guarded log read").await }, if log_read.is_some() => {
                    match logs {
                        Ok(logs) => { let _ = state.messages.send(logs); }
                        Err(error) => tracing::warn!(%error, "reading logs failed"),
                    }
                    log_read = None;
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let state = state.clone();
                            let tls = tls.clone();
                            let idle = config.idle_timeout;
                            connections.spawn(async move {
                                let io = IdleIo::new(stream, idle);
                                let http_idle_enabled = io.enabled.clone();
                                if let Some(tls) = tls {
                                    match tls.accept(io).await {
                                        Ok(stream) => connection(stream, state, None, http_idle_enabled).await,
                                        Err(error) => tracing::debug!(%error, "TLS connection failed"),
                                    }
                                } else { connection(io, state, None, http_idle_enabled).await; }
                            });
                        }
                        Err(error) => { tracing::error!(%error, "accept failed"); break; }
                    }
                }
                accepted = async { redirect.as_ref().expect("guarded redirect").accept().await }, if redirect.is_some() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let state = state.clone();
                            let idle = config.idle_timeout;
                            let port = config.redirect.as_ref().expect("redirect config").https_port;
                            connections.spawn(async move {
                                let io = IdleIo::new(stream, idle);
                                let http_idle_enabled = io.enabled.clone();
                                connection(io, state, Some(port), http_idle_enabled).await;
                            });
                        }
                        Err(error) => { tracing::error!(%error, "redirect accept failed"); break; }
                    }
                }
            }
        }
        drop(watcher);
        drop(listener);
        drop(redirect);
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    });
    Ok(ServerHandle {
        addr,
        redirect_addr,
        shutdown,
        messages,
        task: Some(task),
    })
}

struct State {
    router: Arc<Router>,
    options: ServerOptions,
    messages: broadcast::Sender<String>,
    tls: bool,
    max_request_body_size: usize,
    websockets: bool,
}

async fn connection<T>(
    io: T,
    state: Arc<State>,
    redirect: Option<u16>,
    http_idle_enabled: Arc<AtomicBool>,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (upgrade_tx, mut upgrade_rx) = mpsc::channel::<(OnUpgrade, broadcast::Receiver<String>)>(1);
    let service_state = state.clone();
    let service = service_fn(move |request: http::Request<Incoming>| {
        let state = service_state.clone();
        let upgrade_tx = upgrade_tx.clone();
        async move {
            let mut request = request.map(|body| {
                Body::new(http_body_util::Limited::new(
                    body,
                    state.max_request_body_size,
                ))
            });
            let result = match absolute_uri(&mut request, state.tls && redirect.is_none()) {
                Err(()) => response(400, "text/plain", "Invalid request URL"),
                Ok(()) => {
                    if let Some(port) = redirect {
                        redirect_response(&request, port)
                    } else if state.websockets && websocket_key(&request).is_some() {
                        let key = websocket_key(&request).expect("validated upgrade");
                        let mut result = response(101, "", Body::empty());
                        result.headers_mut().insert(
                            http::header::UPGRADE,
                            http::HeaderValue::from_static("websocket"),
                        );
                        result.headers_mut().insert(
                            http::header::CONNECTION,
                            http::HeaderValue::from_static("Upgrade"),
                        );
                        result.headers_mut().insert(
                            "sec-websocket-accept",
                            http::HeaderValue::from_str(&derive_accept_key(key.as_bytes()))
                                .expect("accept key"),
                        );
                        let receiver = state.messages.subscribe();
                        let upgrade = hyper::upgrade::on(&mut request);
                        let _ = upgrade_tx.send((upgrade, receiver)).await;
                        result
                    } else {
                        let context = url::Url::parse(&request.uri().to_string()).ok().map(|url| {
                            AccessLogEntry {
                                method: request.method().clone(),
                                url,
                                headers: request.headers().clone(),
                            }
                        });
                        let start = Instant::now();
                        let result = state.router.handle_request(request).await;
                        if let (Some(log), Some(context)) = (&state.options.access_log, context) {
                            log(start.elapsed(), &context, &result);
                        }
                        result
                    }
                }
            };
            let mut result = result;
            if !result.headers().contains_key(http::header::CONTENT_TYPE)
                && result.body().size_hint().upper() != Some(0)
            {
                let kind = result
                    .extensions()
                    .get::<crate::BodyKind>()
                    .copied()
                    .unwrap_or(crate::BodyKind::Binary);
                result.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    http::HeaderValue::from_static(kind.content_type()),
                );
            }
            Ok::<_, Infallible>(result)
        }
    });
    if let Err(error) = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(io), service)
        .with_upgrades()
        .await
    {
        tracing::debug!(%error, "HTTP connection ended");
    }
    if let Ok((upgrade, mut messages)) = upgrade_rx.try_recv() {
        match upgrade.await {
            Ok(upgraded) => {
                // The HTTP timer lives below TLS and must stop governing the upgraded transport.
                http_idle_enabled.store(false, Ordering::Relaxed);
                let mut ws =
                    WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None)
                        .await;
                let idle_timeout = Duration::from_secs(120);
                let ping_interval = idle_timeout / 2;
                let idle = sleep(idle_timeout);
                let ping = sleep(ping_interval);
                tokio::pin!(idle, ping);
                loop {
                    tokio::select! {
                        _ = &mut idle => {
                            let _ = ws.close(None).await;
                            break;
                        }
                        event = ws.next() => {
                            if matches!(event, Some(Ok(_))) {
                                idle.as_mut().reset(tokio::time::Instant::now() + idle_timeout);
                                ping.as_mut().reset(tokio::time::Instant::now() + ping_interval);
                            }
                            match event {
                                Some(Ok(Message::Text(_) | Message::Binary(_))) => {
                                    if let Some(read) = &state.options.read_logs {
                                        match read().await {
                                            Ok(logs) => if ws.send(Message::Text(logs.into())).await.is_err() { break; },
                                            Err(error) => tracing::warn!(%error, "reading logs failed"),
                                        }
                                    }
                                }
                                Some(Ok(Message::Ping(_))) => if ws.flush().await.is_err() { break; },
                                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                                _ => {}
                            }
                        },
                        _ = &mut ping => {
                            if ws.send(Message::Ping(bytes::Bytes::new())).await.is_err() { break; }
                            // A silent peer must reach the idle deadline before another ping is due.
                            ping.as_mut().reset(tokio::time::Instant::now() + idle_timeout);
                        },
                        message = messages.recv() => match message {
                            Ok(message) => if ws.send(Message::Text(message.into())).await.is_err() { break; },
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
            Err(error) => tracing::debug!(%error, "WebSocket upgrade failed"),
        }
    }
}

fn absolute_uri(request: &mut Request, tls: bool) -> Result<(), ()> {
    let host = request
        .headers()
        .get(http::header::HOST)
        .map(|host| host.to_str().map_err(|_| ()))
        .transpose()?
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        })
        .unwrap_or("localhost");
    host.parse::<http::uri::Authority>().map_err(|_| ())?;
    if host.contains('@') {
        return Err(());
    }
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let uri = format!("{}://{host}{path}", if tls { "https" } else { "http" })
        .parse()
        .map_err(|_| ())?;
    *request.uri_mut() = uri;
    Ok(())
}

fn redirect_response(request: &Request, port: u16) -> Response {
    let Ok(mut url) = url::Url::parse(&request.uri().to_string()) else {
        return response(400, "text/plain", "Invalid request URL");
    };
    if url.set_scheme("https").is_err() || url.set_port(Some(port)).is_err() {
        return response(400, "text/plain", "Invalid redirect URL");
    }
    let mut result = response(301, "", Body::empty());
    match http::HeaderValue::from_str(url.as_str()) {
        Ok(value) => {
            result.headers_mut().insert(http::header::LOCATION, value);
            result
        }
        Err(_) => response(400, "text/plain", "Invalid redirect URL"),
    }
}

fn websocket_key(request: &Request) -> Option<&str> {
    let headers = request.headers();
    if request.method() != http::Method::GET
        || !headers
            .get(http::header::UPGRADE)?
            .to_str()
            .ok()?
            .eq_ignore_ascii_case("websocket")
        || !headers
            .get(http::header::CONNECTION)?
            .to_str()
            .ok()?
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        || headers.get("sec-websocket-version")? != "13"
    {
        return None;
    }
    let key = headers.get("sec-websocket-key")?.to_str().ok()?;
    // A WebSocket nonce is a base64-encoded 16-byte value.
    if key.len() != 24
        || !key.ends_with("==")
        || !key[..22]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
    {
        return None;
    }
    Some(key)
}

fn tls_acceptor(https: &Https) -> Result<TlsAcceptor, ServerError> {
    let config = match https {
        Https::Pem { cert, key } => tls_config(cert, key)?,
        Https::Generate { domain, cache_path } => {
            if fs::symlink_metadata(cache_path)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "certificate cache is a symlink",
                )
                .into());
            }
            let cached = match fs::read_to_string(cache_path) {
                Ok(pem) => tls_config(&pem, &pem).ok(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if let Some(config) = cached {
                restrict_permissions(cache_path)?;
                config
            } else {
                let generated = rcgen::generate_simple_self_signed(vec![domain.clone()])
                    .map_err(|e| ServerError::Tls(e.to_string()))?;
                let cert = generated.cert.pem();
                let key = generated.signing_key.serialize_pem();
                let config = tls_config(&cert, &key)?;
                write_certificate_cache(cache_path, &format!("{key}\n{cert}"))?;
                config
            }
        }
    };
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn write_certificate_cache(path: &PathBuf, pem: &str) -> io::Result<()> {
    // Exclusive temporary creation and rename avoid exposing partial keys or following symlinks.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temporary = path.with_extension(format!("{}.{nonce}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        file.write_all(pem.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        if let Err(error) = fs::remove_file(&temporary) {
            tracing::warn!(%error, "removing incomplete certificate cache failed");
        }
    }
    result
}

fn restrict_permissions(path: &PathBuf) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn tls_config(cert: &str, key: &str) -> Result<rustls::ServerConfig, ServerError> {
    let certs = rustls_pemfile::certs(&mut cert.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err(ServerError::Tls("missing certificate".into()));
    }
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key.as_bytes())?
        .ok_or_else(|| ServerError::Tls("missing private key".into()))?;
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| ServerError::Tls(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| ServerError::Tls(e.to_string()))
}

/// A timer follows actual transport activity, including partial HTTP and TLS handshakes.
struct IdleIo<T> {
    io: Pin<Box<T>>,
    timer: Pin<Box<Sleep>>,
    timeout: Duration,
    enabled: Arc<AtomicBool>,
}
impl<T> IdleIo<T> {
    fn new(io: T, timeout: Duration) -> Self {
        Self {
            io: Box::pin(io),
            timer: Box::pin(sleep(timeout)),
            timeout,
            enabled: Arc::new(AtomicBool::new(true)),
        }
    }
    fn activity(&mut self) {
        self.timer
            .as_mut()
            .reset(tokio::time::Instant::now() + self.timeout);
    }
    fn expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.enabled.load(Ordering::Relaxed)
            && !self.timeout.is_zero()
            && self.timer.as_mut().poll(cx).is_ready()
    }
}
impl<T: AsyncRead> AsyncRead for IdleIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.expired(cx) {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "idle timeout")));
        }
        let before = buf.filled().len();
        let result = self.io.as_mut().poll_read(cx, buf);
        if buf.filled().len() > before {
            self.activity();
        }
        result
    }
}
impl<T: AsyncWrite> AsyncWrite for IdleIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.expired(cx) {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "idle timeout")));
        }
        let result = self.io.as_mut().poll_write(cx, buf);
        if matches!(result, Poll::Ready(Ok(n)) if n > 0) {
            self.activity();
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.io.as_mut().poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.io.as_mut().poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::{LogLevel, init_file_logger, log_file_change_watcher, read_logfile};
    use crate::{Method, RegisteredRoute};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::time::timeout;
    use tokio_tungstenite::{client_async, connect_async};

    const BUDGET: Duration = Duration::from_secs(3);

    fn config() -> ServerConfig {
        ServerConfig {
            hostname: "127.0.0.1".into(),
            port: Some(0),
            redirect: None,
            ..ServerConfig::default()
        }
    }
    fn router() -> Arc<Router> {
        let mut router = Router::new();
        router
            .add_route(RegisteredRoute {
                method: Method::Get,
                path: "/hello".into(),
                prefixes: vec![],
                handler: Arc::new(|request, _| {
                    Box::pin(async move {
                        response(
                            200,
                            "text/plain",
                            format!(
                                "{} {}",
                                request.uri(),
                                request.headers()["x-test"].to_str().unwrap()
                            ),
                        )
                    })
                }),
            })
            .unwrap();
        Arc::new(router)
    }
    async fn http_over<T: AsyncRead + AsyncWrite + Unpin>(mut stream: T, request: &str) -> String {
        timeout(BUDGET, async {
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut result = Vec::new();
            stream.read_to_end(&mut result).await.unwrap();
            String::from_utf8(result).unwrap()
        })
        .await
        .unwrap()
    }
    async fn http(addr: SocketAddr, path: &str, host: &str) -> String {
        let stream = timeout(BUDGET, TcpStream::connect(addr))
            .await
            .unwrap()
            .unwrap();
        http_over(stream, &format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nX-Test: preserved\r\nConnection: close\r\n\r\n")).await
    }
    async fn tls_stream(addr: SocketAddr, pem: &str) -> tokio_rustls::client::TlsStream<TcpStream> {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut pem.as_bytes()) {
            roots.add(cert.unwrap()).unwrap();
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        timeout(
            BUDGET,
            connector.connect(
                rustls::pki_types::ServerName::try_from("localhost").unwrap(),
                TcpStream::connect(addr).await.unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap()
    }
    async fn assert_closed(mut stream: TcpStream) {
        let mut byte = [0];
        let result = timeout(BUDGET, stream.read(&mut byte)).await.unwrap();
        assert!(
            matches!(result, Ok(0) | Err(_)),
            "socket remained open: {result:?}"
        );
    }

    #[tokio::test]
    async fn incoming_body_limit_counts_declared_and_chunked_bytes_without_pre_auth_buffering() {
        assert_eq!(
            ServerConfig::default().max_request_body_size,
            128 * 1024 * 1024
        );
        let consumer = Arc::new(Router::with_default(Arc::new(|request, _| {
            Box::pin(async move {
                if request.uri().path() == "/reject" {
                    return response(401, "text/plain", "Unauthorized");
                }
                match axum::body::to_bytes(request.into_body(), usize::MAX).await {
                    Ok(bytes) => response(200, "text/plain", bytes),
                    Err(error) => {
                        let mut source: &(dyn std::error::Error + 'static) = &error;
                        loop {
                            if source.is::<http_body_util::LengthLimitError>() {
                                return response(413, "text/plain", "Body too large");
                            }
                            match source.source() {
                                Some(next) => source = next,
                                None => panic!("missing LengthLimitError: {error}"),
                            }
                        }
                    }
                }
            })
        })));
        let mut conf = config();
        conf.max_request_body_size = 8;
        let mut handle = serve(conf.clone(), consumer.clone(), ServerOptions::default())
            .await
            .unwrap();
        let mut overflow_statuses = Vec::new();
        for (headers, body) in [
            ("Content-Length: 9", "abcdefghi"),
            (
                "Transfer-Encoding: chunked",
                "4\r\nabcd\r\n4\r\nefgh\r\n1\r\ni\r\n0\r\n\r\n",
            ),
        ] {
            let result = http_over(
                TcpStream::connect(handle.local_addr()).await.unwrap(),
                &format!("POST /consume HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n\r\n{body}"),
            ).await;
            overflow_statuses.push(result.lines().next().unwrap().to_owned());
        }
        assert_eq!(overflow_statuses, ["HTTP/1.1 413 Payload Too Large"; 2]);
        let exact = http_over(
            TcpStream::connect(handle.local_addr()).await.unwrap(),
            "POST /consume HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 8\r\n\r\néééé",
        ).await;
        assert!(exact.starts_with("HTTP/1.1 200 OK\r\n"), "{exact}");
        assert!(exact.ends_with("éééé"), "{exact}");
        let rejected = http_over(
            TcpStream::connect(handle.local_addr()).await.unwrap(),
            "POST /reject HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 999\r\n\r\n",
        ).await;
        assert!(
            rejected.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
            "{rejected}"
        );
        assert!(rejected.ends_with("Unauthorized"), "{rejected}");
        handle.stop().await.unwrap();

        conf.max_request_body_size = 0;
        let mut handle = serve(conf, consumer, ServerOptions::default())
            .await
            .unwrap();
        for (body, status) in [("", "200 OK"), ("a", "413 Payload Too Large")] {
            let result = http_over(
                TcpStream::connect(handle.local_addr()).await.unwrap(),
                &format!("POST /consume HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()),
            ).await;
            assert!(
                result.starts_with(&format!("HTTP/1.1 {status}\r\n")),
                "{result}"
            );
        }
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn live_transport_infers_text_and_binary_mime_without_changing_memory_headers() {
        let plain = response(404, "", "<script>untrusted</script>");
        assert!(!plain.headers().contains_key(http::header::CONTENT_TYPE));
        let mut routes = Router::new();
        routes
            .add_route(RegisteredRoute {
                method: Method::Get,
                path: "/bytes".into(),
                prefixes: vec![],
                handler: Arc::new(|_, _| {
                    Box::pin(async { response(200, "", bytes::Bytes::from_static(b"hi")) })
                }),
            })
            .unwrap();
        routes
            .add_route(RegisteredRoute {
                method: Method::Get,
                path: "/empty".into(),
                prefixes: vec![],
                handler: Arc::new(|_, _| Box::pin(async { response(200, "", Body::empty()) })),
            })
            .unwrap();
        let mut handle = serve(
            config(),
            Arc::new(routes),
            ServerOptions {
                access_log: None,
                ..ServerOptions::default()
            },
        )
        .await
        .unwrap();
        let missing = http(handle.local_addr(), "/missing", "localhost").await;
        assert!(
            missing.contains("\r\ncontent-type: text/plain;charset=utf-8\r\n"),
            "{missing}"
        );
        let binary = http(handle.local_addr(), "/bytes", "localhost").await;
        assert!(
            binary.contains("\r\ncontent-type: application/octet-stream\r\n"),
            "{binary}"
        );
        let empty = http(handle.local_addr(), "/empty", "localhost").await;
        assert!(!empty.contains("content-type:"), "{empty}");
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn http_only_upgrade_dispatches_and_logs_missing_and_existing_routes() {
        let (tx, mut logs) = mpsc::unbounded_channel();
        let options = ServerOptions {
            access_log: Some(Arc::new(move |_, context, response| {
                tx.send((context.clone(), response.status())).unwrap();
            })),
            read_logs: None,
            watch_logs: None,
        };
        let mut handle = serve_http_only(config(), router(), options).await.unwrap();
        for (path, status, body) in [
            ("/__missing__", 404, "Not found - Route not defined"),
            ("/hello", 200, "http://example.test/hello preserved"),
        ] {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut stream = TcpStream::connect(handle.local_addr()).await.unwrap();
            stream.write_all(format!(
                "GET {path} HTTP/1.1\r\nHost: example.test\r\nX-Test: preserved\r\nConnection: Upgrade, close\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
            ).as_bytes()).await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut headers = String::new();
            timeout(BUDGET, async {
                loop {
                    let mut line = String::new();
                    assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                    headers.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
            })
            .await
            .unwrap();
            assert!(
                headers.starts_with(&format!("HTTP/1.1 {status} ")),
                "{headers}"
            );
            assert!(
                !headers
                    .to_ascii_lowercase()
                    .contains("sec-websocket-accept:")
            );
            let mut result = String::new();
            timeout(BUDGET, stream.read_to_string(&mut result))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result, body);
            let (context, logged_status) = timeout(BUDGET, logs.recv()).await.unwrap().unwrap();
            assert_eq!(context.method, http::Method::GET);
            assert_eq!(context.url.as_str(), format!("http://example.test{path}"));
            assert_eq!(context.headers["upgrade"], "websocket");
            assert_eq!(logged_status, status);
        }
        timeout(BUDGET, handle.stop()).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn default_serve_still_upgrades_missing_paths_without_log_delivery() {
        let options = ServerOptions {
            access_log: None,
            read_logs: None,
            watch_logs: None,
        };
        let mut handle = serve(config(), router(), options).await.unwrap();
        let (mut ws, handshake) = timeout(
            BUDGET,
            connect_async(format!("ws://{}/__missing__", handle.local_addr())),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(handshake.status(), 101);
        handle.reload();
        assert_eq!(
            timeout(BUDGET, ws.next()).await.unwrap().unwrap().unwrap(),
            Message::Text("RELOAD".into())
        );
        timeout(BUDGET, handle.stop()).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn http_access_log_websocket_reload_and_stop_release_open_connections() {
        let (tx, mut logs) = mpsc::unbounded_channel();
        let options = ServerOptions {
            access_log: Some(Arc::new(move |_, context, response| {
                tx.send((context.clone(), response.status())).unwrap();
            })),
            ..ServerOptions::default()
        };
        let mut handle = serve(config(), router(), options).await.unwrap();
        let addr = handle.local_addr();
        let result = http(addr, "/hello?x=1", "custom.example:123").await;
        assert!(result.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(result.ends_with("http://custom.example:123/hello?x=1 preserved"));
        let (context, status) = timeout(BUDGET, logs.recv()).await.unwrap().unwrap();
        assert_eq!(context.method, http::Method::GET);
        assert_eq!(context.url.as_str(), "http://custom.example:123/hello?x=1");
        assert_eq!(context.headers["x-test"], "preserved");
        assert_eq!(status, 200);
        let trace = http_over(
            TcpStream::connect(addr).await.unwrap(),
            "TRACE /hello HTTP/1.1\r\nHost: custom.example\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(trace.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        let (context, status) = timeout(BUDGET, logs.recv()).await.unwrap().unwrap();
        assert_eq!(context.method, http::Method::TRACE);
        assert_eq!(context.url.as_str(), "http://custom.example/hello");
        assert_eq!(status, 405);
        let (mut ws1, handshake) =
            timeout(BUDGET, connect_async(format!("ws://{addr}/arbitrary?x=1")))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(handshake.status(), 101);
        let (mut ws2, _) = timeout(BUDGET, connect_async(format!("ws://{addr}/another")))
            .await
            .unwrap()
            .unwrap();
        handle.reload();
        for ws in [&mut ws1, &mut ws2] {
            assert_eq!(
                timeout(BUDGET, ws.next()).await.unwrap().unwrap().unwrap(),
                Message::Text("RELOAD".into())
            );
        }
        let mut partial = TcpStream::connect(addr).await.unwrap();
        partial
            .write_all(b"GET /hello HTTP/1.1\r\nHost:")
            .await
            .unwrap();
        timeout(BUDGET, handle.stop()).await.unwrap().unwrap();
        for ws in [&mut ws1, &mut ws2] {
            let closed = timeout(BUDGET, ws.next()).await.unwrap();
            assert!(matches!(
                closed,
                None | Some(Err(_)) | Some(Ok(Message::Close(_)))
            ));
        }
        assert_closed(partial).await;
        let rebound = TcpListener::bind(addr).await.unwrap();
        assert_eq!(rebound.local_addr().unwrap(), addr);
        handle.stop().await.unwrap();
    }

    async fn advance_network_clock(duration: Duration) {
        // Real sockets need the clock running while awaiting OS readiness.
        tokio::time::pause();
        tokio::time::advance(duration).await;
        tokio::time::resume();
    }

    async fn websocket_ready<T: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<T>) {
        ws.send(Message::Ping(b"ready".to_vec().into()))
            .await
            .unwrap();
        assert_eq!(
            timeout(BUDGET, ws.next()).await.unwrap().unwrap().unwrap(),
            Message::Pong(b"ready".to_vec().into())
        );
    }

    async fn delayed_reload<T: AsyncRead + AsyncWrite + Unpin>(
        handle: &ServerHandle,
        ws: &mut WebSocketStream<T>,
    ) {
        websocket_ready(ws).await;
        advance_network_clock(Duration::from_secs(11)).await;
        handle.reload();
        assert_eq!(
            timeout(BUDGET, ws.next()).await.unwrap().unwrap().unwrap(),
            Message::Text("RELOAD".into())
        );
    }

    #[tokio::test]
    async fn quiet_websocket_survives_http_idle_and_receives_delayed_reload() {
        let mut handle = serve(config(), router(), ServerOptions::default())
            .await
            .unwrap();
        let (mut ws, _) = connect_async(format!("ws://{}/reload", handle.local_addr()))
            .await
            .unwrap();
        delayed_reload(&handle, &mut ws).await;
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn quiet_tls_websocket_survives_http_idle_and_receives_delayed_reload() {
        let generated = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert = generated.cert.pem();
        let mut conf = config();
        conf.https = Some(Https::Pem {
            cert: cert.clone(),
            key: generated.signing_key.serialize_pem(),
        });
        let mut handle = serve(conf, router(), ServerOptions::default())
            .await
            .unwrap();
        let (mut ws, _) = client_async(
            "wss://localhost/reload",
            tls_stream(handle.local_addr(), &cert).await,
        )
        .await
        .unwrap();
        delayed_reload(&handle, &mut ws).await;
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn websocket_automatic_pings_keep_responsive_peers_alive_past_120_seconds() {
        let mut conf = config();
        conf.idle_timeout = Duration::ZERO;
        let mut handle = serve(conf, router(), ServerOptions::default())
            .await
            .unwrap();
        let (mut ws, _) = connect_async(format!("ws://{}/reload", handle.local_addr()))
            .await
            .unwrap();
        websocket_ready(&mut ws).await;
        for _ in 0..3 {
            advance_network_clock(Duration::from_secs(60)).await;
            assert_eq!(
                timeout(BUDGET, ws.next()).await.unwrap().unwrap().unwrap(),
                Message::Ping(bytes::Bytes::new())
            );
            ws.flush().await.unwrap();
            websocket_ready(&mut ws).await;
        }
        handle.reload();
        assert_eq!(
            timeout(BUDGET, ws.next()).await.unwrap().unwrap().unwrap(),
            Message::Text("RELOAD".into())
        );
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn websocket_idle_timeout_closes_unresponsive_peer_at_120_seconds() {
        let mut conf = config();
        conf.idle_timeout = Duration::ZERO;
        let mut handle = serve(conf, router(), ServerOptions::default())
            .await
            .unwrap();
        let (mut ws, _) = connect_async(format!("ws://{}/reload", handle.local_addr()))
            .await
            .unwrap();
        websocket_ready(&mut ws).await;
        advance_network_clock(Duration::from_secs(60)).await;
        assert_eq!(
            timeout(BUDGET, ws.next()).await.unwrap().unwrap().unwrap(),
            Message::Ping(bytes::Bytes::new())
        );
        // Leave the automatically queued pong unsent to model an unresponsive peer.
        advance_network_clock(Duration::from_secs(60)).await;
        assert!(matches!(
            timeout(BUDGET, ws.next()).await.unwrap(),
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None
        ));
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn websocket_reads_logs_on_messages_and_broadcasts_real_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let logger = init_file_logger(&path).unwrap();
        logger
            .log(LogLevel::Info, &[serde_json::json!("initial")])
            .unwrap();
        let read_path = path.clone();
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();
        struct ObservedWatcher {
            inner: Box<dyn LogWatcher>,
            closed: mpsc::UnboundedSender<()>,
        }
        impl LogWatcher for ObservedWatcher {
            fn close(&mut self) {
                self.inner.close();
                let _ = self.closed.send(());
            }
        }
        let options = ServerOptions {
            access_log: None,
            read_logs: Some(Arc::new(move || {
                let path = read_path.clone();
                Box::pin(async move { read_logfile(path, 80).await })
            })),
            watch_logs: Some(Arc::new(move |change| {
                let watcher = log_file_change_watcher(&path, change).map_err(io::Error::other)?;
                Ok(Box::new(ObservedWatcher {
                    inner: Box::new(watcher),
                    closed: closed_tx.clone(),
                }))
            })),
        };
        let mut handle = serve(config(), router(), options).await.unwrap();
        let (mut ws1, _) = timeout(
            BUDGET,
            connect_async(format!("ws://{}/logs", handle.local_addr())),
        )
        .await
        .unwrap()
        .unwrap();
        let (mut ws2, _) = timeout(
            BUDGET,
            connect_async(format!("ws://{}/logs2", handle.local_addr())),
        )
        .await
        .unwrap()
        .unwrap();
        ws1.send(Message::Text("anything".into())).await.unwrap();
        let initial = timeout(BUDGET, ws1.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        assert!(initial.ends_with("][I] - \"initial\""));
        ws2.send(Message::Binary(vec![1].into())).await.unwrap();
        assert_eq!(
            timeout(BUDGET, ws2.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap(),
            initial
        );
        logger
            .log(LogLevel::Warn, &[serde_json::json!("live update")])
            .unwrap();
        for ws in [&mut ws1, &mut ws2] {
            let logs = timeout(BUDGET, ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap();
            assert!(logs.starts_with(initial.as_str()));
            assert!(logs.ends_with("][W] - \"live update\""));
        }
        timeout(BUDGET, handle.stop()).await.unwrap().unwrap();
        assert_eq!(closed_rx.recv().await, Some(()));
        assert_eq!(
            closed_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        );
    }

    #[tokio::test]
    async fn generated_tls_is_trusted_cached_private_and_redirect_is_configurable() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cert.pem");
        fs::write(&cache, "truncated").unwrap();
        let mut conf = config();
        conf.https = Some(Https::Generate {
            domain: "localhost".into(),
            cache_path: cache.clone(),
        });
        conf.redirect = Some(Redirect {
            port: 0,
            ..Redirect::default()
        });
        let mut handle = serve(conf.clone(), router(), ServerOptions::default())
            .await
            .unwrap();
        let pem = fs::read_to_string(&cache).unwrap();
        assert!(pem.contains("PRIVATE KEY") && pem.contains("CERTIFICATE"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let secure = http_over(tls_stream(handle.local_addr(), &pem).await,
            "GET /hello?tls=1 HTTP/1.1\r\nHost: secure.example:8443\r\nX-Test: secure\r\nConnection: close\r\n\r\n").await;
        assert!(secure.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(secure.ends_with("https://secure.example:8443/hello?tls=1 secure"));
        let redir_addr = handle.redirect_addr().unwrap();
        let mut partial_redirect = TcpStream::connect(redir_addr).await.unwrap();
        partial_redirect
            .write_all(b"GET / HTTP/1.1\r\nHost:")
            .await
            .unwrap();
        let redirect = http(redir_addr, "/hello?x=1", "localhost:1234").await;
        assert!(redirect.starts_with("HTTP/1.1 301 Moved Permanently\r\n"));
        assert!(redirect.contains("location: https://localhost/hello?x=1\r\n"));
        let (mut ws, upgrade) = timeout(
            BUDGET,
            client_async(
                "wss://localhost/tls-websocket",
                tls_stream(handle.local_addr(), &pem).await,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(upgrade.status(), 101);
        handle.reload();
        assert_eq!(
            timeout(BUDGET, ws.next()).await.unwrap().unwrap().unwrap(),
            Message::Text("RELOAD".into())
        );
        let mut stalled_tls = TcpStream::connect(handle.local_addr()).await.unwrap();
        stalled_tls.write_all(&[22, 3]).await.unwrap();
        let addr = handle.local_addr();
        timeout(BUDGET, handle.stop()).await.unwrap().unwrap();
        assert_closed(stalled_tls).await;
        assert_closed(partial_redirect).await;
        assert!(TcpListener::bind(addr).await.is_ok());
        assert!(TcpListener::bind(redir_addr).await.is_ok());
        conf.redirect.as_mut().unwrap().https_port = 9443;
        let mut cached = serve(conf, router(), ServerOptions::default())
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&cache).unwrap(), pem);
        assert!(
            http(cached.redirect_addr().unwrap(), "/hello", "localhost:12")
                .await
                .contains("location: https://localhost:9443/hello\r\n")
        );
        cached.stop().await.unwrap();
    }

    #[tokio::test]
    async fn explicit_pem_hosts_tls_without_a_redirect_and_invalid_pem_fails() {
        let generated = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert = generated.cert.pem();
        let mut conf = config();
        conf.https = Some(Https::Pem {
            cert: cert.clone(),
            key: generated.signing_key.serialize_pem(),
        });
        let mut handle = serve(conf, router(), ServerOptions::default())
            .await
            .unwrap();
        assert_eq!(handle.redirect_addr(), None);
        let result = http_over(
            tls_stream(handle.local_addr(), &cert).await,
            "GET /hello HTTP/1.1\r\nHost: localhost\r\nX-Test: pem\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(result.ends_with("https://localhost/hello pem"));
        handle.stop().await.unwrap();
        let mut invalid = config();
        invalid.https = Some(Https::Pem {
            cert,
            key: "not a key".into(),
        });
        let error = serve(invalid, router(), ServerOptions::default())
            .await
            .err()
            .unwrap();
        assert!(matches!(error, ServerError::Tls(ref message) if message == "missing private key"));
    }

    #[tokio::test]
    async fn failed_start_closes_watcher_and_releases_main_listener() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let main_addr = probe.local_addr().unwrap();
        drop(probe);
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        struct CloseNotice(mpsc::UnboundedSender<()>);
        impl LogWatcher for CloseNotice {
            fn close(&mut self) {
                self.0.send(()).unwrap();
            }
        }
        let options = || ServerOptions {
            access_log: None,
            read_logs: Some(Arc::new(|| Box::pin(async { Ok("logs".into()) }))),
            watch_logs: Some(Arc::new({
                let tx = tx.clone();
                move |_| Ok(Box::new(CloseNotice(tx.clone())))
            })),
        };
        let mut conf = config();
        conf.port = Some(occupied.local_addr().unwrap().port());
        let error = serve(conf, router(), options()).await.err().unwrap();
        assert!(
            matches!(error, ServerError::Io(ref error) if error.kind() == io::ErrorKind::AddrInUse)
        );
        assert_eq!(timeout(BUDGET, rx.recv()).await.unwrap(), Some(()));
        let mut conf = config();
        conf.port = Some(main_addr.port());
        conf.https = Some(Https::Generate {
            domain: "localhost".into(),
            cache_path: dir.path().join("cert"),
        });
        conf.redirect = Some(Redirect {
            port: occupied.local_addr().unwrap().port(),
            https_port: 443,
        });
        let error = serve(conf, router(), options()).await.err().unwrap();
        assert!(
            matches!(error, ServerError::Io(ref error) if error.kind() == io::ErrorKind::AddrInUse)
        );
        assert_eq!(timeout(BUDGET, rx.recv()).await.unwrap(), Some(()));
        assert!(TcpListener::bind(main_addr).await.is_ok());
    }

    #[tokio::test]
    async fn stop_cancels_pending_handlers_and_drop_stops_listener() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pending_router = Arc::new(Router::with_default(Arc::new(move |_, _| {
            tx.send(()).unwrap();
            Box::pin(std::future::pending())
        })));
        let mut handle = serve(config(), pending_router, ServerOptions::default())
            .await
            .unwrap();
        let mut stream = TcpStream::connect(handle.local_addr()).await.unwrap();
        stream
            .write_all(b"GET /pending HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(timeout(BUDGET, rx.recv()).await.unwrap(), Some(()));
        timeout(BUDGET, handle.stop()).await.unwrap().unwrap();
        assert_closed(stream).await;
        let handle = serve(config(), router(), ServerOptions::default())
            .await
            .unwrap();
        let addr = handle.local_addr();
        let stream = TcpStream::connect(addr).await.unwrap();
        drop(handle);
        assert_closed(stream).await;
        assert!(TcpListener::bind(addr).await.is_ok());
    }

    #[tokio::test]
    async fn idle_timeout_closes_partial_http_and_tls_handshakes() {
        let mut conf = config();
        conf.idle_timeout = Duration::from_millis(40);
        let mut handle = serve(conf.clone(), router(), ServerOptions::default())
            .await
            .unwrap();
        let mut stream = TcpStream::connect(handle.local_addr()).await.unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        assert_closed(stream).await;
        handle.stop().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        conf.https = Some(Https::Generate {
            domain: "localhost".into(),
            cache_path: dir.path().join("cert"),
        });
        let mut handle = serve(conf, router(), ServerOptions::default())
            .await
            .unwrap();
        let mut stream = TcpStream::connect(handle.local_addr()).await.unwrap();
        stream.write_all(&[22, 3]).await.unwrap();
        assert_closed(stream).await;
        handle.stop().await.unwrap();
    }
}
