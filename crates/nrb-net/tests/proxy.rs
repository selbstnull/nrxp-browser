//! Локальный CONNECT-прокси на `SystemTransport`: авторизация, политика приватных адресов, ошибки, перекачка.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use nrb_net::proxy::{LocalProxy, ProxyPolicy};
use nrb_transport::SystemTransport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = socket.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

async fn proxy(allow_private: bool) -> LocalProxy {
    LocalProxy::start(Arc::new(SystemTransport), ProxyPolicy { allow_private })
        .await
        .unwrap()
}

/// Из `http://user:pass@host:port` достаёт значение заголовка Proxy-Authorization.
fn auth_header(proxy: &LocalProxy) -> String {
    let url = proxy.url();
    let creds = url
        .trim_start_matches("http://")
        .split('@')
        .next()
        .unwrap()
        .to_owned();
    let (user, pass) = creds.split_once(':').unwrap();
    let encoded = {
        // тот же base64, что и в прокси, но через независимую реализацию
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let data = format!("{user}:{pass}").into_bytes();
        let mut out = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16
                | (*c.get(1).unwrap_or(&0) as u32) << 8
                | *c.get(2).unwrap_or(&0) as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 {
                T[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if c.len() > 2 {
                T[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    };
    format!("Basic {encoded}")
}

async fn send(proxy: &LocalProxy, request: &str) -> (String, TcpStream) {
    let mut socket = TcpStream::connect(proxy.addr()).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut byte))
            .await
            .expect("прокси не ответил")
            .unwrap();
        if n == 0 {
            break;
        }
        head.push(byte[0]);
    }
    (String::from_utf8_lossy(&head).into_owned(), socket)
}

#[tokio::test]
async fn connect_with_credentials_tunnels_bytes() {
    let target = echo().await;
    let proxy = proxy(true).await;
    let request = format!(
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {}\r\n\r\n",
        auth_header(&proxy)
    );
    let (head, mut socket) = send(&proxy, &request).await;
    assert!(head.starts_with("HTTP/1.1 200"), "ответ: {head}");

    let payload: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();
    let (mut reader, mut writer) = socket.split();
    let write = async { writer.write_all(&payload).await.unwrap() };
    let mut got = vec![0u8; payload.len()];
    let read = async { reader.read_exact(&mut got).await.unwrap() };
    tokio::join!(write, read);
    assert!(got == payload);
    assert!(
        proxy
            .stats()
            .total
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 1
    );
}

#[tokio::test]
async fn missing_or_wrong_credentials_get_407() {
    let target = echo().await;
    let proxy = proxy(true).await;
    let (head, _) = send(&proxy, &format!("CONNECT {target} HTTP/1.1\r\n\r\n")).await;
    assert!(head.starts_with("HTTP/1.1 407"), "ответ: {head}");
    let (head, _) = send(
        &proxy,
        &format!(
            "CONNECT {target} HTTP/1.1\r\nProxy-Authorization: Basic d3Jvbmc6d3Jvbmc=\r\n\r\n"
        ),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 407"), "ответ: {head}");
}

#[tokio::test]
async fn private_targets_are_forbidden_by_default() {
    let target = echo().await; // 127.0.0.1
    let proxy = proxy(false).await;
    let request = format!(
        "CONNECT {target} HTTP/1.1\r\nProxy-Authorization: {}\r\n\r\n",
        auth_header(&proxy)
    );
    let (head, _) = send(&proxy, &request).await;
    assert!(head.starts_with("HTTP/1.1 403"), "ответ: {head}");
    for host in [
        "localhost:80",
        "169.254.169.254:80",
        "192.168.1.1:80",
        "[::1]:80",
    ] {
        let request = format!(
            "CONNECT {host} HTTP/1.1\r\nProxy-Authorization: {}\r\n\r\n",
            auth_header(&proxy)
        );
        let (head, _) = send(&proxy, &request).await;
        assert!(head.starts_with("HTTP/1.1 403"), "{host}: {head}");
    }
}

#[tokio::test]
async fn non_connect_and_bad_requests_are_rejected() {
    let proxy = proxy(true).await;
    let auth = auth_header(&proxy);
    let (head, _) = send(
        &proxy,
        &format!("GET http://example.org/ HTTP/1.1\r\nProxy-Authorization: {auth}\r\n\r\n"),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 405"), "ответ: {head}");
    let (head, _) = send(
        &proxy,
        &format!("CONNECT nonsense HTTP/1.1\r\nProxy-Authorization: {auth}\r\n\r\n"),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 400"), "ответ: {head}");
}

#[tokio::test]
async fn unreachable_target_gets_502() {
    let proxy = proxy(true).await;
    let dead = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap()
    };
    let request = format!(
        "CONNECT {dead} HTTP/1.1\r\nProxy-Authorization: {}\r\n\r\n",
        auth_header(&proxy)
    );
    let (head, _) = send(&proxy, &request).await;
    assert!(head.starts_with("HTTP/1.1 502"), "ответ: {head}");
}

#[tokio::test]
async fn proxy_is_loopback_only() {
    let proxy = proxy(true).await;
    assert!(proxy.addr().ip().is_loopback());
}
