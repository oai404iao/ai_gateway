//! Proxy-aware TCP/TLS setup for upstream WebSocket handshakes.

use std::{
    borrow::Cow,
    collections::VecDeque,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
    time::Duration,
};

use axum::http::HeaderMap;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{StreamExt, stream::FuturesUnordered};
use reqwest::Url;
use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::TcpStream,
    time::{Instant, sleep_until, timeout},
};
use tokio_rustls::TlsConnector;
use tokio_socks::{
    TargetAddr,
    tcp::{Socks4Stream, Socks5Stream},
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, client_async_tls_with_config,
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig},
};
use zeroize::Zeroizing;

use crate::domain::{CompiledChannelUpstreamPolicy, CompiledProxy};

use super::UpstreamWebSocketError;

const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(250);
const MAX_PROXY_RESPONSE_HEADER_BYTES: usize = 16 * 1024;

pub(super) type UpstreamStream = WebSocketStream<MaybeTlsStream<Box<dyn AsyncIo>>>;

pub(super) async fn connect(
    target: Url,
    headers: HeaderMap,
    channel: &CompiledChannelUpstreamPolicy,
    connect_timeout: Duration,
    config: WebSocketConfig,
) -> Result<UpstreamStream, UpstreamWebSocketError> {
    let host = target
        .host_str()
        .ok_or(UpstreamWebSocketError::InvalidConfiguration)?;
    let port = target
        .port_or_known_default()
        .ok_or(UpstreamWebSocketError::InvalidConfiguration)?;
    let stream = match channel.proxy().map(Arc::as_ref) {
        Some(proxy) if !proxy_bypasses_target(proxy, host) => {
            connect_via_proxy(proxy, host, port, connect_timeout).await?
        }
        Some(_) | None => {
            Box::new(connect_tcp(host, port, connect_timeout).await?) as Box<dyn AsyncIo>
        }
    };

    let mut request = target
        .as_str()
        .into_client_request()
        .map_err(|_| UpstreamWebSocketError::InvalidConfiguration)?;
    request.headers_mut().extend(headers);
    let connector = Connector::Rustls(tls_config());
    let (stream, _) = client_async_tls_with_config(request, stream, Some(config), Some(connector))
        .await
        .map_err(map_websocket_error)?;
    Ok(stream)
}

fn proxy_bypasses_target(proxy: &CompiledProxy, host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    proxy
        .no_proxy_hosts()
        .iter()
        .any(|rule| rule.matches_host(host))
}

async fn connect_via_proxy(
    proxy: &CompiledProxy,
    target_host: &str,
    target_port: u16,
    connect_timeout: Duration,
) -> Result<Box<dyn AsyncIo>, UpstreamWebSocketError> {
    match proxy.url().scheme() {
        "http" | "https" => {
            connect_http_proxy(proxy, target_host, target_port, connect_timeout).await
        }
        "socks4" | "socks4a" | "socks5" | "socks5h" => {
            connect_socks_proxy(proxy, target_host, target_port, connect_timeout).await
        }
        _ => Err(UpstreamWebSocketError::InvalidConfiguration),
    }
}

async fn connect_http_proxy(
    proxy: &CompiledProxy,
    target_host: &str,
    target_port: u16,
    connect_timeout: Duration,
) -> Result<Box<dyn AsyncIo>, UpstreamWebSocketError> {
    let proxy_host = proxy
        .url()
        .host_str()
        .ok_or(UpstreamWebSocketError::InvalidConfiguration)?;
    let proxy_port = proxy
        .url()
        .port_or_known_default()
        .ok_or(UpstreamWebSocketError::InvalidConfiguration)?;
    let stream = connect_tcp(proxy_host, proxy_port, connect_timeout).await?;
    let stream: Box<dyn AsyncIo> = if proxy.url().scheme() == "https" {
        let server_name = ServerName::try_from(
            proxy_host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .unwrap_or(proxy_host)
                .to_owned(),
        )
        .map_err(|_| UpstreamWebSocketError::InvalidConfiguration)?;
        let tls = TlsConnector::from(tls_config())
            .connect(server_name, stream)
            .await
            .map_err(|_| UpstreamWebSocketError::Network)?;
        Box::new(tls)
    } else {
        Box::new(stream)
    };
    establish_http_tunnel(stream, proxy, target_host, target_port).await
}

async fn establish_http_tunnel(
    mut stream: Box<dyn AsyncIo>,
    proxy: &CompiledProxy,
    target_host: &str,
    target_port: u16,
) -> Result<Box<dyn AsyncIo>, UpstreamWebSocketError> {
    let authority = host_port(target_host, target_port);
    let mut request = Zeroizing::new(format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    ));
    if proxy.username().is_some() || proxy.password().is_some() {
        let credentials = Zeroizing::new(format!(
            "{}:{}",
            proxy.username().unwrap_or_default(),
            proxy.password().unwrap_or_default()
        ));
        let encoded = Zeroizing::new(STANDARD.encode(credentials.as_bytes()));
        request.push_str("Proxy-Authorization: Basic ");
        request.push_str(&encoded);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| UpstreamWebSocketError::Network)?;
    stream
        .flush()
        .await
        .map_err(|_| UpstreamWebSocketError::Network)?;

    let mut received = Vec::with_capacity(1024);
    let header_end = loop {
        if received.len() >= MAX_PROXY_RESPONSE_HEADER_BYTES {
            return Err(UpstreamWebSocketError::Network);
        }
        let mut chunk = [0_u8; 1024];
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|_| UpstreamWebSocketError::Network)?;
        if read == 0 {
            return Err(UpstreamWebSocketError::Network);
        }
        received.extend_from_slice(&chunk[..read]);
        if let Some(end) = received
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
        {
            break end;
        }
    };
    let status_line_end = received
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or(UpstreamWebSocketError::Network)?;
    let status_line = std::str::from_utf8(&received[..status_line_end])
        .map_err(|_| UpstreamWebSocketError::Network)?;
    let status = status_line
        .split_ascii_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or(UpstreamWebSocketError::Network)?;
    if status != 200 {
        return Err(UpstreamWebSocketError::Network);
    }
    let prefix = received.split_off(header_end);
    if prefix.is_empty() {
        Ok(stream)
    } else {
        Ok(Box::new(PrefixedIo::new(prefix, stream)))
    }
}

async fn connect_socks_proxy(
    proxy: &CompiledProxy,
    target_host: &str,
    target_port: u16,
    connect_timeout: Duration,
) -> Result<Box<dyn AsyncIo>, UpstreamWebSocketError> {
    let proxy_host = proxy
        .url()
        .host_str()
        .ok_or(UpstreamWebSocketError::InvalidConfiguration)?;
    let proxy_port = proxy
        .url()
        .port_or_known_default()
        .ok_or(UpstreamWebSocketError::InvalidConfiguration)?;
    let socket = connect_tcp(proxy_host, proxy_port, connect_timeout).await?;
    let remote_dns = matches!(proxy.url().scheme(), "socks4a" | "socks5h");
    let normalized_target_host = target_host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(target_host);
    let target = if let Ok(address) = normalized_target_host.parse() {
        TargetAddr::Ip(SocketAddr::new(address, target_port))
    } else if remote_dns {
        TargetAddr::Domain(Cow::Owned(target_host.to_owned()), target_port)
    } else {
        TargetAddr::Ip(resolve_target(target_host, target_port, connect_timeout).await?)
    };

    match proxy.url().scheme() {
        "socks4" | "socks4a" => {
            if proxy.username().is_some() || proxy.password().is_some() {
                return Err(UpstreamWebSocketError::InvalidConfiguration);
            }
            Socks4Stream::connect_with_socket(socket, target)
                .await
                .map(|stream| Box::new(stream) as Box<dyn AsyncIo>)
                .map_err(|_| UpstreamWebSocketError::Network)
        }
        "socks5" | "socks5h" => match (proxy.username(), proxy.password()) {
            (None, None) => Socks5Stream::connect_with_socket(socket, target)
                .await
                .map(|stream| Box::new(stream) as Box<dyn AsyncIo>)
                .map_err(|_| UpstreamWebSocketError::Network),
            (Some(username), Some(password)) => {
                Socks5Stream::connect_with_password_and_socket(socket, target, username, password)
                    .await
                    .map(|stream| Box::new(stream) as Box<dyn AsyncIo>)
                    .map_err(|_| UpstreamWebSocketError::Network)
            }
            _ => Err(UpstreamWebSocketError::InvalidConfiguration),
        },
        _ => Err(UpstreamWebSocketError::InvalidConfiguration),
    }
}

async fn resolve_target(
    host: &str,
    port: u16,
    connect_timeout: Duration,
) -> Result<SocketAddr, UpstreamWebSocketError> {
    timeout(
        connect_timeout,
        tokio::net::lookup_host(host_port(host, port)),
    )
    .await
    .map_err(|_| UpstreamWebSocketError::ConnectTimeout)?
    .map_err(|_| UpstreamWebSocketError::Network)?
    .next()
    .ok_or(UpstreamWebSocketError::Network)
}

async fn connect_tcp(
    host: &str,
    port: u16,
    connect_timeout: Duration,
) -> Result<TcpStream, UpstreamWebSocketError> {
    let stream = timeout(connect_timeout, async {
        let addresses = tokio::net::lookup_host(host_port(host, port))
            .await?
            .collect::<Vec<_>>();
        connect_happy_eyeballs(addresses, TcpStream::connect).await
    })
    .await
    .map_err(|_| UpstreamWebSocketError::ConnectTimeout)?
    .map_err(|_| UpstreamWebSocketError::Network)?;
    stream
        .set_nodelay(true)
        .map_err(|_| UpstreamWebSocketError::Network)?;
    Ok(stream)
}

async fn connect_happy_eyeballs<T, F, Fut>(
    addresses: Vec<SocketAddr>,
    mut connect: F,
) -> io::Result<T>
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let mut addresses = addresses.into_iter();
    let Some(first_address) = addresses.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "could not resolve websocket target",
        ));
    };
    let first_is_ipv4 = first_address.is_ipv4();
    let mut preferred = VecDeque::new();
    let mut alternate = VecDeque::new();
    for address in addresses {
        if address.is_ipv4() == first_is_ipv4 {
            preferred.push_back(address);
        } else {
            alternate.push_back(address);
        }
    }
    let mut addresses = VecDeque::new();
    while !preferred.is_empty() || !alternate.is_empty() {
        if let Some(address) = alternate.pop_front() {
            addresses.push_back(address);
        }
        if let Some(address) = preferred.pop_front() {
            addresses.push_back(address);
        }
    }

    let mut attempts = FuturesUnordered::new();
    attempts.push(connect(first_address));
    let mut next_attempt_at = Instant::now() + HAPPY_EYEBALLS_DELAY;
    let mut last_error = None;
    loop {
        if addresses.is_empty() {
            match attempts.next().await {
                Some(Ok(stream)) => return Ok(stream),
                Some(Err(error)) => {
                    if attempts.is_empty() {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
                None => {
                    return Err(last_error.unwrap_or_else(|| {
                        io::Error::other("websocket connection attempts ended")
                    }));
                }
            }
            continue;
        }
        tokio::select! {
            result = attempts.next() => {
                match result {
                    Some(Ok(stream)) => return Ok(stream),
                    Some(Err(error)) => {
                        last_error = Some(error);
                        attempts.push(connect(take_next_address(&mut addresses)?));
                        next_attempt_at = Instant::now() + HAPPY_EYEBALLS_DELAY;
                    }
                    None => {
                        attempts.push(connect(take_next_address(&mut addresses)?));
                        next_attempt_at = Instant::now() + HAPPY_EYEBALLS_DELAY;
                    }
                }
            }
            _ = sleep_until(next_attempt_at) => {
                attempts.push(connect(take_next_address(&mut addresses)?));
                next_attempt_at = Instant::now() + HAPPY_EYEBALLS_DELAY;
            }
        }
    }
}

fn take_next_address(addresses: &mut VecDeque<SocketAddr>) -> io::Result<SocketAddr> {
    addresses
        .pop_front()
        .ok_or_else(|| io::Error::other("websocket address queue is empty"))
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }))
}

fn map_websocket_error(error: tokio_tungstenite::tungstenite::Error) -> UpstreamWebSocketError {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => UpstreamWebSocketError::Http {
            status: response.status().as_u16(),
        },
        tokio_tungstenite::tungstenite::Error::ConnectionClosed
        | tokio_tungstenite::tungstenite::Error::AlreadyClosed => UpstreamWebSocketError::Closed,
        _ => UpstreamWebSocketError::Network,
    }
}

pub(super) trait AsyncIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> AsyncIo for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

struct PrefixedIo<S> {
    prefix: Vec<u8>,
    offset: usize,
    inner: S,
}

impl<S> PrefixedIo<S> {
    fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            offset: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedIo<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.offset < self.prefix.len() {
            let available = &self.prefix[self.offset..];
            let length = available.len().min(buffer.remaining());
            buffer.put_slice(&available[..length]);
            self.offset += length;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedIo<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}
