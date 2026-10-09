//! Загрузки на локальном сервере: имя из заголовка, куки только исходному хосту, 403 без куки, отмена, `.part`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nrb_net::Fetcher;
use nrb_net::download::{DownloadRequest, DownloadState, Downloads};
use nrb_transport::SystemTransport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let port = addr.port();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let has_cookie = request.to_lowercase().contains("cookie: sid=42");
                let response: Vec<u8> = match path.as_str() {
                    "/private" if !has_cookie => b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                    "/private" => {
                        let body = b"secret-bytes";
                        let mut out = format!("HTTP/1.1 200 OK\r\ncontent-disposition: attachment; filename=\"../../my report.txt\"\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).into_bytes();
                        out.extend_from_slice(body);
                        out
                    }
                    "/to-other-host" => format!("HTTP/1.1 302 Found\r\nlocation: http://localhost:{port}/echo-cookie\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").into_bytes(),
                    "/echo-cookie" => {
                        let body = if has_cookie { "COOKIE-LEAKED" } else { "no-cookie" };
                        format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).into_bytes()
                    }
                    "/slow" => {
                        let head = b"HTTP/1.1 200 OK\r\ncontent-length: 1000000\r\nconnection: close\r\n\r\n";
                        let _ = socket.write_all(head).await;
                        for _ in 0..1000 {
                            if socket.write_all(&[b'z'; 1000]).await.is_err() {
                                return;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        return;
                    }
                    _ => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                };
                let _ = socket.write_all(&response).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    addr
}

fn setup() -> (Arc<Fetcher>, Arc<Downloads>, std::path::PathBuf) {
    let fetcher =
        Arc::new(Fetcher::new(Arc::new(SystemTransport)).with_timeout(Duration::from_secs(30)));
    let dir = std::env::temp_dir().join(format!(
        "nrb-dl-it-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    (fetcher, Downloads::new(), dir)
}

async fn wait_finished(downloads: &Downloads, id: u64) -> nrb_net::download::DownloadInfo {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let info = downloads.list().into_iter().find(|d| d.id == id).unwrap();
        if info.state != DownloadState::Running {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "загрузка не завершилась: {info:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cookie_is_sent_and_name_comes_from_the_header() {
    let addr = server().await;
    let (fetcher, downloads, dir) = setup();
    let id = downloads
        .start(
            fetcher,
            &tokio::runtime::Handle::current(),
            DownloadRequest {
                url: Some(format!("http://{addr}/private").parse().unwrap()),
                dest_dir: dir.clone(),
                cookies: Some("sid=42".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let info = wait_finished(&downloads, id).await;
    assert_eq!(info.state, DownloadState::Done, "{info:?}");
    assert_eq!(
        info.file_name, "my report.txt",
        "путь из заголовка не должен выходить из каталога"
    );
    let path = info.path.unwrap();
    assert!(path.starts_with(&dir));
    assert_eq!(std::fs::read(&path).unwrap(), b"secret-bytes");
    assert!(
        !dir.join("my report.txt.part").exists(),
        ".part переименован"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn without_cookie_the_server_refuses_and_no_file_is_left() {
    let addr = server().await;
    let (fetcher, downloads, dir) = setup();
    let id = downloads
        .start(
            fetcher,
            &tokio::runtime::Handle::current(),
            DownloadRequest {
                url: Some(format!("http://{addr}/private").parse().unwrap()),
                dest_dir: dir.clone(),
                ..Default::default()
            },
        )
        .unwrap();
    let info = wait_finished(&downloads, id).await;
    assert!(
        matches!(info.state, DownloadState::Failed(ref m) if m.contains("403")),
        "{info:?}"
    );
    assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn cookie_is_not_forwarded_to_another_host_after_redirect() {
    let addr = server().await;
    let (fetcher, downloads, dir) = setup();
    let id = downloads
        .start(
            fetcher,
            &tokio::runtime::Handle::current(),
            DownloadRequest {
                url: Some(
                    format!("http://127.0.0.1:{}/to-other-host", addr.port())
                        .parse()
                        .unwrap(),
                ),
                dest_dir: dir.clone(),
                cookies: Some("sid=42".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let info = wait_finished(&downloads, id).await;
    assert_eq!(info.state, DownloadState::Done, "{info:?}");
    assert_eq!(
        std::fs::read(info.path.unwrap()).unwrap(),
        b"no-cookie",
        "куки ушли на чужой хост"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_the_download_and_removes_the_part_file() {
    let addr = server().await;
    let (fetcher, downloads, dir) = setup();
    let id = downloads
        .start(
            fetcher,
            &tokio::runtime::Handle::current(),
            DownloadRequest {
                url: Some(format!("http://{addr}/slow").parse().unwrap()),
                dest_dir: dir.clone(),
                ..Default::default()
            },
        )
        .unwrap();
    // дождаться начала записи
    let deadline = Instant::now() + Duration::from_secs(10);
    while downloads.list()[0].received == 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    downloads.cancel(id);
    let info = wait_finished(&downloads, id).await;
    assert_eq!(info.state, DownloadState::Cancelled, "{info:?}");
    assert!(
        std::fs::read_dir(&dir).unwrap().next().is_none(),
        "остался файл после отмены"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
