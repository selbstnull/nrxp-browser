//! HTTP-клиент на локальном сервере по `SystemTransport` (без интернета): тело, редирект, потоковая запись, ошибки.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use nrb_net::{FetchError, Fetcher};
use nrb_transport::SystemTransport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

/// Минимальный HTTP/1.1-сервер: маршрутизация по пути запроса, каждый ответ закрывает соединение.
async fn server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let response: Vec<u8> = match path.as_str() {
                    "/hello" => b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
                    "/redirect" => b"HTTP/1.1 302 Found\r\nlocation: /hello\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                    "/loop" => b"HTTP/1.1 302 Found\r\nlocation: /loop\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                    "/missing" => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                    "/big" => {
                        let body = vec![b'x'; 3_000_000];
                        let mut out = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).into_bytes();
                        out.extend_from_slice(&body);
                        out
                    }
                    "/echo-ua" => {
                        let ua = request.lines().find(|l| l.to_lowercase().starts_with("user-agent:")).unwrap_or("").to_owned();
                        format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{ua}", ua.len()).into_bytes()
                    }
                    _ => b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                };
                let _ = socket.write_all(&response).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    addr
}

fn fetcher() -> Fetcher {
    Fetcher::new(Arc::new(SystemTransport)).with_timeout(Duration::from_secs(10))
}

fn url(addr: SocketAddr, path: &str) -> Url {
    format!("http://{addr}{path}").parse().unwrap()
}

#[tokio::test]
async fn plain_get() {
    let addr = server().await;
    let response = fetcher().get(&url(addr, "/hello")).await.unwrap();
    assert_eq!((response.status, response.version), (200, "http/1.1"));
    assert_eq!(&response.body[..], b"hello");
}

#[tokio::test]
async fn follows_redirects_and_reports_them() {
    let addr = server().await;
    let response = fetcher().get(&url(addr, "/redirect")).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.redirects, 1);
    assert_eq!(response.final_url.path(), "/hello");
}

#[tokio::test]
async fn redirect_loop_is_an_error() {
    let addr = server().await;
    assert!(matches!(
        fetcher().get(&url(addr, "/loop")).await,
        Err(FetchError::TooManyRedirects)
    ));
}

#[tokio::test]
async fn http_errors_are_responses_not_failures() {
    let addr = server().await;
    assert_eq!(
        fetcher().get(&url(addr, "/missing")).await.unwrap().status,
        404
    );
}

#[tokio::test]
async fn streaming_does_not_buffer_the_body() {
    let addr = server().await;
    let mut total = 0usize;
    let mut sink = |chunk: &[u8]| {
        total += chunk.len();
        Ok(())
    };
    let response = fetcher()
        .get_streaming(&url(addr, "/big"), &mut sink)
        .await
        .unwrap();
    assert!(
        response.body.is_empty(),
        "в потоковом режиме тело не копится"
    );
    assert_eq!(total, 3_000_000);
}

#[tokio::test]
async fn sends_identifying_user_agent() {
    let addr = server().await;
    let response = fetcher().get(&url(addr, "/echo-ua")).await.unwrap();
    assert!(String::from_utf8_lossy(&response.body).contains("nrb/"));
}

#[tokio::test]
async fn unsupported_scheme_and_refused_connection() {
    assert!(matches!(
        fetcher().get(&"ftp://example.org/".parse().unwrap()).await,
        Err(FetchError::Scheme(_))
    ));
    let dead = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap()
    };
    assert!(matches!(
        fetcher().get(&url(dead, "/")).await,
        Err(FetchError::Connect { .. })
    ));
}
