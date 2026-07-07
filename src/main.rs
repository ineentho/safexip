mod api;
mod config;
mod dns;
mod state;
mod xip;

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tokio::net::{TcpListener, UdpSocket};
use tracing_subscriber::EnvFilter;

use config::Config;
use dns::DnsHandler;
use state::AcmeRecords;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("safexip=info")),
        )
        .init();

    let config = Config::parse();
    let acme = AcmeRecords::new();

    tracing::info!("starting safexip for domain {}", config.domain);
    tracing::info!("NS: {} -> {}", config.ns_hostname, config.ns_ip);
    tracing::info!("DNS on {}:{}", config.dns_bind, config.dns_port);

    let handler = Arc::new(DnsHandler {
        config: config.clone(),
        acme: acme.clone(),
    });

    // UDP DNS listener
    let udp_addr: SocketAddr = format!("{}:{}", config.dns_bind, config.dns_port).parse()?;
    let udp_sock = UdpSocket::bind(udp_addr).await?;
    tracing::info!("UDP DNS listening on {udp_addr}");

    let udp_handler = handler.clone();
    tokio::spawn(async move {
        run_udp_dns(udp_sock, udp_handler).await;
    });

    // TCP DNS listener
    let tcp_listener = TcpListener::bind(udp_addr).await?;
    tracing::info!("TCP DNS listening on {udp_addr}");

    tokio::spawn(async move {
        run_tcp_dns(tcp_listener, handler).await;
    });

    // HTTP API
    let api_addr: SocketAddr = format!("{}:{}", config.api_bind, config.api_port).parse()?;
    tracing::info!("API listening on {api_addr}");

    let app = api::router(config.clone(), acme);
    let listener = tokio::net::TcpListener::bind(api_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
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
        let response = handler.handle(&data).await;
        if !response.is_empty() {
            if let Err(e) = sock.send_to(&response, src).await {
                tracing::error!("UDP send error: {e}");
            }
        }
    }
}

async fn run_tcp_dns(listener: TcpListener, handler: Arc<DnsHandler>) {
    loop {
        let (mut stream, addr) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("TCP accept error: {e}");
                continue;
            }
        };
        let handler = handler.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;

            let mut len_buf = [0u8; 2];
            if let Err(e) = stream.read_exact(&mut len_buf).await {
                tracing::debug!("TCP read length from {addr}: {e}");
                return;
            }
            let msg_len = u16::from_be_bytes(len_buf) as usize;
            let mut msg_buf = vec![0u8; msg_len];
            if let Err(e) = stream.read_exact(&mut msg_buf).await {
                tracing::debug!("TCP read msg from {addr}: {e}");
                return;
            }

            let response = handler.handle(&msg_buf).await;
            let resp_len = (response.len() as u16).to_be_bytes();
            if let Err(e) = stream.write_all(&[resp_len[0], resp_len[1]]).await {
                tracing::debug!("TCP write len to {addr}: {e}");
                return;
            }
            if let Err(e) = stream.write_all(&response).await {
                tracing::debug!("TCP write msg to {addr}: {e}");
            }
        });
    }
}
