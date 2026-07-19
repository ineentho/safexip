use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};

const API_KEY: &str = "0123456789abcdef0123456789abcdef";
const DEADLINE: Duration = Duration::from_secs(5);

struct Server {
    child: Child,
    dns_port: u16,
    api_port: u16,
}

impl Server {
    fn start(max_tokens: usize) -> Self {
        let dns_port = reserve_dns_port();
        let api_port = reserve_tcp_port();
        let child = Command::new(env!("CARGO_BIN_EXE_safexip"))
            .env("SAFEXIP_DOMAIN", "xip.test")
            .env("SAFEXIP_NS_HOSTNAME", "ns1.xip.test")
            .env("SAFEXIP_NS_HOSTNAME2", "ns2.xip.test")
            .env("SAFEXIP_NS_IP", "127.0.0.1")
            .env("SAFEXIP_DNS_BIND", "127.0.0.1")
            .env("SAFEXIP_DNS_PORT", dns_port.to_string())
            .env("SAFEXIP_API_BIND", "127.0.0.1")
            .env("SAFEXIP_API_PORT", api_port.to_string())
            .env("SAFEXIP_API_KEY", API_KEY)
            .env("SAFEXIP_TOKEN_LIFETIME", "1")
            .env("SAFEXIP_MAX_TOKENS", max_tokens.to_string())
            .env("RUST_LOG", "safexip=debug")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start safexip");
        let mut server = Self {
            child,
            dns_port,
            api_port,
        };
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Ok(response) = server.http("GET", "/health", None, None) {
                if response.starts_with("HTTP/1.1 200")
                    && response.contains(r#"{"status":"ok","domain":"xip.test"}"#)
                {
                    break;
                }
            }
            if let Some(status) = server.child.try_wait().expect("inspect child") {
                panic!("safexip exited before readiness: {status}");
            }
            assert!(Instant::now() < deadline, "safexip readiness timed out");
            thread::sleep(Duration::from_millis(20));
        }
        server
    }

    fn dns_addr(&self) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.dns_port)
    }

    fn http(
        &self,
        method: &str,
        path: &str,
        auth: Option<&str>,
        body: Option<&str>,
    ) -> std::io::Result<String> {
        let mut stream = TcpStream::connect_timeout(
            &SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.api_port).into(),
            DEADLINE,
        )?;
        stream.set_read_timeout(Some(DEADLINE))?;
        let body = body.unwrap_or("");
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(auth) = auth {
            request.push_str(&format!("Authorization: {auth}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream.write_all(request.as_bytes())?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    }

    fn shutdown(mut self) {
        let status = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .expect("send SIGTERM");
        assert!(status.success());
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(status) = self.child.try_wait().expect("wait for safexip") {
                assert!(status.success(), "SIGTERM exit was {status}");
                std::mem::forget(self);
                return;
            }
            assert!(Instant::now() < deadline, "SIGTERM shutdown timed out");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reserve_tcp_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn reserve_dns_port() -> u16 {
    for _ in 0..20 {
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = udp.local_addr().unwrap().port();
        if let Ok(tcp) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            drop(tcp);
            drop(udp);
            return port;
        }
    }
    panic!("could not reserve a UDP/TCP DNS port");
}

fn query(name: &str, record_type: RecordType) -> Vec<u8> {
    let mut message = Message::new(42, MessageType::Query, OpCode::Query);
    message.add_query(Query::query(Name::from_ascii(name).unwrap(), record_type));
    message.to_vec().unwrap()
}

fn udp_query(server: &Server, name: &str, record_type: RecordType) -> Message {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.set_read_timeout(Some(DEADLINE)).unwrap();
    socket
        .send_to(&query(name, record_type), server.dns_addr())
        .unwrap();
    let mut response = [0; 4096];
    let length = socket.recv(&mut response).unwrap();
    Message::from_vec(&response[..length]).unwrap()
}

fn write_tcp_query(stream: &mut TcpStream, raw: &[u8]) {
    stream.write_all(&(raw.len() as u16).to_be_bytes()).unwrap();
    stream.write_all(raw).unwrap();
}

fn read_tcp_response(stream: &mut TcpStream) -> Message {
    let mut length = [0; 2];
    stream.read_exact(&mut length).unwrap();
    let mut response = vec![0; u16::from_be_bytes(length) as usize];
    stream.read_exact(&mut response).unwrap();
    Message::from_vec(&response).unwrap()
}

fn auth(password: &str) -> String {
    let value = base64::engine::general_purpose::STANDARD.encode(format!("test:{password}"));
    format!("Basic {value}")
}

fn challenge(value: &str) -> String {
    format!(r#"{{"fqdn":"_acme-challenge.xip.test.","value":"{value}"}}"#)
}

#[test]
fn real_udp_tcp_api_expiration_and_shutdown() {
    let server = Server::start(10);

    let a = udp_query(&server, "127-0-0-1.xip.test", RecordType::A);
    assert_eq!(a.metadata.response_code, ResponseCode::NoError);
    assert!(matches!(a.answers[0].data, RData::A(_)));

    let mut tcp = TcpStream::connect(server.dns_addr()).unwrap();
    tcp.set_read_timeout(Some(DEADLINE)).unwrap();
    for record_type in [RecordType::SOA, RecordType::NS] {
        write_tcp_query(&mut tcp, &query("xip.test", record_type));
        let response = read_tcp_response(&mut tcp);
        assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        assert!(!response.answers.is_empty());
        assert!(response
            .answers
            .iter()
            .all(|record| record.name.to_ascii() == "xip.test."));

        write_tcp_query(&mut tcp, &query("127-0-0-1.xip.test", record_type));
        let response = read_tcp_response(&mut tcp);
        assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        assert!(response.metadata.authoritative);
        assert!(response.answers.is_empty());
        assert_eq!(response.authorities.len(), 1);
        assert!(matches!(response.authorities[0].data, RData::SOA(_)));
        assert_eq!(response.authorities[0].name.to_ascii(), "xip.test.");
        assert!(response.additionals.is_empty());

        write_tcp_query(&mut tcp, &query("missing.xip.test", record_type));
        let response = read_tcp_response(&mut tcp);
        assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);
        assert!(response.metadata.authoritative);
        assert!(response.answers.is_empty());
        assert_eq!(response.authorities.len(), 1);
        assert!(matches!(response.authorities[0].data, RData::SOA(_)));
        assert_eq!(response.authorities[0].name.to_ascii(), "xip.test.");
        assert!(response.additionals.is_empty());

        write_tcp_query(&mut tcp, &query("example.com", record_type));
        let response = read_tcp_response(&mut tcp);
        assert_eq!(response.metadata.response_code, ResponseCode::Refused);
        assert!(!response.metadata.authoritative);
        assert!(response.answers.is_empty());
        assert!(response.authorities.is_empty());
        assert!(response.additionals.is_empty());
    }

    let missing = server
        .http("POST", "/present", None, Some(&challenge("one")))
        .unwrap();
    assert!(missing.starts_with("HTTP/1.1 401"));
    let wrong = server
        .http(
            "POST",
            "/present",
            Some(&auth("wrong")),
            Some(&challenge("one")),
        )
        .unwrap();
    assert!(wrong.starts_with("HTTP/1.1 401"));

    let present = server
        .http(
            "POST",
            "/present",
            Some(&auth(API_KEY)),
            Some(&challenge("one")),
        )
        .unwrap();
    assert!(present.starts_with("HTTP/1.1 200"));
    assert_eq!(
        udp_query(&server, "_acme-challenge.xip.test", RecordType::TXT)
            .answers
            .len(),
        1
    );

    let cleanup = server
        .http(
            "POST",
            "/cleanup",
            Some(&auth(API_KEY)),
            Some(&challenge("one")),
        )
        .unwrap();
    assert!(cleanup.starts_with("HTTP/1.1 200"));
    assert!(
        udp_query(&server, "_acme-challenge.xip.test", RecordType::TXT)
            .answers
            .is_empty()
    );

    server
        .http(
            "POST",
            "/present",
            Some(&auth(API_KEY)),
            Some(&challenge("expires")),
        )
        .unwrap();
    thread::sleep(Duration::from_millis(1200));
    assert!(
        udp_query(&server, "_acme-challenge.xip.test", RecordType::TXT)
            .answers
            .is_empty()
    );

    let malformed = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    malformed.send_to(&[0, 1, 2], server.dns_addr()).unwrap();
    assert_eq!(
        udp_query(&server, "xip.test", RecordType::SOA)
            .metadata
            .response_code,
        ResponseCode::NoError
    );
    server.shutdown();
}

#[test]
fn bounded_udp_api_and_token_capacity() {
    let server = Server::start(3);
    for _ in 0..500 {
        let response = udp_query(&server, "127-0-0-1.xip.test", RecordType::A);
        assert_eq!(response.answers.len(), 1);
    }

    let statuses: Vec<String> = thread::scope(|scope| {
        let handles: Vec<_> = (0..12)
            .map(|index| {
                let server = &server;
                scope.spawn(move || {
                    server
                        .http(
                            "POST",
                            "/present",
                            Some(&auth(API_KEY)),
                            Some(&challenge(&format!("token-{index}"))),
                        )
                        .unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    assert_eq!(
        statuses
            .iter()
            .filter(|status| status.starts_with("HTTP/1.1 200"))
            .count(),
        3
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| status.starts_with("HTTP/1.1 503"))
            .count(),
        9
    );
    assert_eq!(
        udp_query(&server, "_acme-challenge.xip.test", RecordType::TXT)
            .answers
            .len(),
        3
    );
    server.shutdown();
}

// This deliberately consumes more file descriptors and is run by the Linux
// abuse-test CI job, not by every local `cargo test` invocation.
#[test]
#[ignore = "run in the dedicated bounded-abuse CI job"]
fn tcp_connection_limit_releases_capacity() {
    let server = Server::start(10);
    let mut held: Vec<TcpStream> = (0..1024)
        .map(|_| TcpStream::connect(server.dns_addr()).unwrap())
        .collect();
    thread::sleep(Duration::from_millis(200));

    let mut overflow = TcpStream::connect(server.dns_addr()).unwrap();
    overflow
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    write_tcp_query(&mut overflow, &query("xip.test", RecordType::SOA));
    let mut byte = [0];
    assert!(overflow.read(&mut byte).is_err());

    held.pop();
    overflow.set_read_timeout(Some(DEADLINE)).unwrap();
    let response = read_tcp_response(&mut overflow);
    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    server.shutdown();
}
