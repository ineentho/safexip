mod api;
mod config;
mod dns;
mod state;
mod wire;
mod xip;

use std::error::Error;
use std::io::ErrorKind;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tower::ServiceExt;
use tracing_subscriber::EnvFilter;

use config::Config;
use dns::DnsHandler;
use state::AcmeRecords;

const MAX_HTTP_CONNECTIONS: usize = 128;
const HTTP_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("safexip=info")),
        )
        .init();

    let config = Config::parse();
    config
        .validate()
        .map_err(|error| format!("invalid configuration: {error}"))?;
    let acme = AcmeRecords::new(
        config.token_lifetime(),
        config.max_tokens,
        wire::AcmeWireCapacity::from_config(&config),
    );

    tracing::info!("starting safexip for domain {}", config.domain);
    tracing::info!("NS: {} -> {}", config.ns_hostname, config.ns_ip);
    tracing::info!("DNS on {}:{}", config.dns_bind, config.dns_port);

    let handler = Arc::new(DnsHandler {
        config: config.clone(),
        acme: acme.clone(),
    });

    // UDP DNS listener
    let udp_addr = SocketAddr::new(config.dns_bind, config.dns_port);
    let udp_sock = UdpSocket::bind(udp_addr).await?;
    tracing::info!("UDP DNS listening on {udp_addr}");

    // TCP DNS listener
    let tcp_listener = TcpListener::bind(udp_addr).await?;
    tracing::info!("TCP DNS listening on {udp_addr}");

    // HTTP API
    let api_addr = SocketAddr::new(config.api_bind, config.api_port);
    tracing::info!("API listening on {api_addr}");

    let app = api::router(config.clone(), acme);
    let listener = tokio::net::TcpListener::bind(api_addr).await?;

    tokio::select! {
        _ = run_udp_dns(udp_sock, handler.clone()) => {
            return Err("UDP DNS listener stopped unexpectedly".into());
        }
        _ = run_tcp_dns(tcp_listener, handler) => {
            return Err("TCP DNS listener stopped unexpectedly".into());
        }
        result = run_http_api(listener, app) => {
            result?;
        }
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received");
        }
    }

    Ok(())
}

async fn run_http_api(listener: TcpListener, app: axum::Router) -> std::io::Result<()> {
    run_http_api_with_limits(
        listener,
        app,
        MAX_HTTP_CONNECTIONS,
        HTTP_HEADER_READ_TIMEOUT,
    )
    .await
}

async fn run_http_api_with_limits(
    listener: TcpListener,
    app: axum::Router,
    max_connections: usize,
    header_read_timeout: Duration,
) -> std::io::Result<()> {
    let permits = Arc::new(Semaphore::new(max_connections));
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::error!("HTTP accept error: {error}");
                continue;
            }
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            tracing::debug!("HTTP connection limit reached; rejecting {addr}");
            drop(stream);
            continue;
        };
        let app = app.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let service = service_fn(move |request| {
                let app = app.clone();
                async move { app.oneshot(request.map(axum::body::Body::new)).await }
            });
            let mut builder = http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(header_read_timeout)
                .max_headers(32);
            if let Err(error) = builder
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                tracing::debug!("HTTP connection from {addr} closed: {error}");
            }
        });
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result {
                            tracing::error!("failed to listen for Ctrl-C: {error}");
                        }
                    }
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::error!("failed to listen for SIGTERM: {error}");
                if let Err(error) = tokio::signal::ctrl_c().await {
                    tracing::error!("failed to listen for Ctrl-C: {error}");
                }
            }
        }
    }

    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!("failed to listen for Ctrl-C: {error}");
    }
}

async fn run_udp_dns(sock: UdpSocket, handler: Arc<DnsHandler>) {
    let mut buf = vec![0u8; 1500];
    loop {
        let (len, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("UDP recv error: {e}");
                continue;
            }
        };
        let data = buf[..len].to_vec();
        let handler = handler.clone();
        let response = handler.handle_udp(&data).await;
        if !response.is_empty() {
            if let Err(e) = sock.send_to(&response, src).await {
                tracing::error!("UDP send error: {e}");
            }
        }
    }
}

async fn run_tcp_dns(listener: TcpListener, handler: Arc<DnsHandler>) {
    const MAX_CONNECTIONS: usize = 1024;
    const READ_TIMEOUT: Duration = Duration::from_secs(30);

    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let permit = match permits.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return,
        };
        let (mut stream, addr) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("TCP accept error: {e}");
                continue;
            }
        };
        let handler = handler.clone();
        tokio::spawn(async move {
            let _permit = permit;
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;

            loop {
                let mut len_buf = [0u8; 2];
                match timeout(READ_TIMEOUT, stream.read_exact(&mut len_buf)).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) if error.kind() == ErrorKind::UnexpectedEof => return,
                    Ok(Err(error)) => {
                        tracing::debug!("TCP read length from {addr}: {error}");
                        return;
                    }
                    Err(_) => {
                        tracing::debug!("TCP connection from {addr} timed out");
                        return;
                    }
                }
                let msg_len = u16::from_be_bytes(len_buf) as usize;
                if msg_len == 0 {
                    return;
                }
                let mut msg_buf = vec![0u8; msg_len];
                match timeout(READ_TIMEOUT, stream.read_exact(&mut msg_buf)).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        tracing::debug!("TCP read message from {addr}: {error}");
                        return;
                    }
                    Err(_) => {
                        tracing::debug!("TCP message from {addr} timed out");
                        return;
                    }
                }

                let response = handler.handle(&msg_buf).await;
                if response.is_empty() {
                    return;
                }
                let Ok(resp_len) = u16::try_from(response.len()) else {
                    tracing::error!("TCP DNS response exceeded 65535 bytes");
                    return;
                };
                if let Err(error) = stream.write_all(&resp_len.to_be_bytes()).await {
                    tracing::debug!("TCP write length to {addr}: {error}");
                    return;
                }
                if let Err(error) = stream.write_all(&response).await {
                    tracing::debug!("TCP write message to {addr}: {error}");
                    return;
                }
            }
        });
    }
}

#[cfg(test)]
mod http_tests {
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn header_timeout_and_connection_limit_release_capacity() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/", get(|| async { "ok" }));
        let server = tokio::spawn(run_http_api_with_limits(
            listener,
            app,
            1,
            Duration::from_millis(50),
        ));

        let mut held = tokio::net::TcpStream::connect(addr).await.unwrap();
        held.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        let mut rejected = tokio::net::TcpStream::connect(addr).await.unwrap();
        rejected
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut byte = [0];
        let read = tokio::time::timeout(Duration::from_millis(200), rejected.read(&mut byte))
            .await
            .expect("over-capacity connection was not closed");
        assert!(read.map_or(true, |length| length == 0));

        let held_read = tokio::time::timeout(Duration::from_millis(200), held.read(&mut byte))
            .await
            .expect("incomplete headers were not timed out");
        assert!(held_read.map_or(true, |length| length == 0));

        let mut recovered = tokio::net::TcpStream::connect(addr).await.unwrap();
        recovered
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(
            Duration::from_millis(200),
            recovered.read_to_end(&mut response),
        )
        .await
        .expect("recovered connection did not respond")
        .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));

        server.abort();
        let _ = server.await;
    }
}
