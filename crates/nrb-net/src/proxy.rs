//! Локальный HTTP-CONNECT прокси для движка (I5): Servo настраивается на него через prefs `network_http(s)_proxy_uri`
//! (другого способа у Servo нет — он умеет только CONNECT), а прокси открывает потоки через [`Transport`].
//!
//! * слушает только `127.0.0.1`, порт случайный;
//! * доступ по Basic-авторизации со случайным паролем на запуск (чтобы другие локальные процессы не могли пользоваться туннелем);
//! * имя хоста уходит транспорту как есть — DNS резолвит выходная нода, локально имена не резолвятся;
//! * по умолчанию запрещены цели loopback/private/link-local (иначе страница могла бы сканировать сеть выходной ноды).

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use nrb_transport::Transport;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const MAX_HEAD: usize = 16 * 1024;

/// Счётчики для индикатора в UI.
#[derive(Debug, Default)]
pub struct ProxyStats {
    pub active: AtomicUsize,
    pub total: AtomicU64,
    pub failed: AtomicU64,
    pub bytes_up: AtomicU64,
    pub bytes_down: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProxyPolicy {
    /// Разрешить цели loopback/private/link-local (нужно для локальной разработки и режима «домашний egress»).
    pub allow_private: bool,
}

pub struct LocalProxy {
    /// Для варианта на unix-сокете (`start_unix`) — заглушка `127.0.0.1:0`; адрес смотрите в `unix_path`.
    addr: SocketAddr,
    unix_path: Option<std::path::PathBuf>,
    username: String,
    password: String,
    stats: Arc<ProxyStats>,
    task: JoinHandle<()>,
}

impl Drop for LocalProxy {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(path) = &self.unix_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 18];
    getrandom::fill(&mut bytes).expect("системный генератор случайных чисел");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl LocalProxy {
    pub async fn start(transport: Arc<dyn Transport>, policy: ProxyPolicy) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let username = format!("nrb-{}", random_token());
        let password = random_token();
        let expected = basic_credentials(&username, &password);
        let stats = Arc::new(ProxyStats::default());

        let task_stats = stats.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, peer)) = listener.accept().await else {
                    return;
                };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let (transport, expected, stats) =
                    (transport.clone(), expected.clone(), task_stats.clone());
                tokio::spawn(async move {
                    let _ = handle(socket, transport, &expected, policy, &stats).await;
                });
            }
        });
        Ok(Self {
            addr,
            unix_path: None,
            username,
            password,
            stats,
            task,
        })
    }

    /// Вариант для герметичного режима (I11): слушает **unix-сокет** (путь доступен и из другого сетевого пространства имён),
    /// а не TCP. Права на сокет — только владельцу (каталог 0700 создаёт вызывающий). Доступ к прокси по-прежнему по
    /// логину и паролю: сокет-файл — не единственная защита.
    #[cfg(unix)]
    pub async fn start_unix(
        path: &std::path::Path,
        transport: Arc<dyn Transport>,
        policy: ProxyPolicy,
    ) -> io::Result<Self> {
        let _ = std::fs::remove_file(path);
        let listener = tokio::net::UnixListener::bind(path)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        let username = format!("nrb-{}", random_token());
        let password = random_token();
        let expected = basic_credentials(&username, &password);
        let stats = Arc::new(ProxyStats::default());
        let task_stats = stats.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let (transport, expected, stats) =
                    (transport.clone(), expected.clone(), task_stats.clone());
                tokio::spawn(async move {
                    let _ = handle(socket, transport, &expected, policy, &stats).await;
                });
            }
        });
        Ok(Self {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            unix_path: Some(path.to_owned()),
            username,
            password,
            stats,
            task,
        })
    }

    pub fn unix_path(&self) -> Option<&std::path::Path> {
        self.unix_path.as_deref()
    }

    /// Логин и пароль прокси (для сборки URL, когда адрес даёт релей, а не сам прокси).
    pub fn credentials(&self) -> (&str, &str) {
        (&self.username, &self.password)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// URL для prefs движка: `http://логин:пароль@127.0.0.1:порт`.
    pub fn url(&self) -> String {
        format!("http://{}:{}@{}", self.username, self.password, self.addr)
    }

    pub fn stats(&self) -> &Arc<ProxyStats> {
        &self.stats
    }
}

fn basic_credentials(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64(format!("{username}:{password}").as_bytes())
    )
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Сравнение без раннего выхода по первому отличию.
fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Цель считаем приватной, если это `localhost`, .local/.internal или IP-литерал из закрытых диапазонов.
pub fn is_private_target(host: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1])
        }
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

/// Обёртка над клиентским сокетом: прочитанное — исходящий трафик браузера, записанное — входящий.
struct Counted<'a, S> {
    inner: S,
    stats: &'a ProxyStats,
}

impl<S: AsyncRead + Unpin> tokio::io::AsyncRead for Counted<'_, S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let before = buf.filled().len();
        let poll = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if let std::task::Poll::Ready(Ok(())) = &poll {
            self.stats
                .bytes_up
                .fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        }
        poll
    }
}

impl<S: AsyncWrite + Unpin> tokio::io::AsyncWrite for Counted<'_, S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        data: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let poll = std::pin::Pin::new(&mut self.inner).poll_write(cx, data);
        if let std::task::Poll::Ready(Ok(n)) = &poll {
            self.stats
                .bytes_down
                .fetch_add(*n as u64, Ordering::Relaxed);
        }
        poll
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Поток, отданный движку напрямую (без CONNECT-прокси): считает трафик в общие [`ProxyStats`]. Для движка чтение — это
/// загрузка (`bytes_down`), запись — отправка (`bytes_up`); счётчик активных соединений уменьшается при закрытии.
pub struct CountedStream<S> {
    inner: S,
    stats: Arc<ProxyStats>,
}

impl<S> CountedStream<S> {
    pub fn new(inner: S, stats: Arc<ProxyStats>) -> Self {
        stats.active.fetch_add(1, Ordering::Relaxed);
        stats.total.fetch_add(1, Ordering::Relaxed);
        Self { inner, stats }
    }
}

impl<S> Drop for CountedStream<S> {
    fn drop(&mut self) {
        self.stats.active.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CountedStream<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let before = buf.filled().len();
        let poll = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if let std::task::Poll::Ready(Ok(())) = &poll {
            self.stats
                .bytes_down
                .fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        }
        poll
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountedStream<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        data: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let poll = std::pin::Pin::new(&mut self.inner).poll_write(cx, data);
        if let std::task::Poll::Ready(Ok(n)) = &poll {
            self.stats.bytes_up.fetch_add(*n as u64, Ordering::Relaxed);
        }
        poll
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct Head {
    method: String,
    target: String,
    auth: Option<String>,
    /// Байты после заголовков (клиент мог прислать данные не дожидаясь 200).
    leftover: Vec<u8>,
}

async fn read_head<S: AsyncRead + Unpin>(socket: &mut S) -> io::Result<Head> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let text = String::from_utf8_lossy(&buf[..end]).into_owned();
            let leftover = buf[end + 4..].to_vec();
            let mut lines = text.lines();
            let mut first = lines.next().unwrap_or("").split_whitespace();
            let (method, target) = (
                first.next().unwrap_or("").to_owned(),
                first.next().unwrap_or("").to_owned(),
            );
            let auth = lines
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.trim().eq_ignore_ascii_case("proxy-authorization"))
                .map(|(_, value)| value.trim().to_owned());
            return Ok(Head {
                method,
                target,
                auth,
                leftover,
            });
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "слишком длинный заголовок",
            ));
        }
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn reply<S: AsyncWrite + Unpin>(socket: &mut S, status: &str, extra: &str) {
    let _ = socket
        .write_all(
            format!("HTTP/1.1 {status}\r\n{extra}content-length: 0\r\nconnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await;
    let _ = socket.shutdown().await;
}

fn split_host_port(target: &str) -> Option<(String, u16)> {
    let (host, port) = target.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    (!host.is_empty()).then(|| {
        (
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned(),
            port,
        )
    })
}

async fn handle<S: AsyncRead + AsyncWrite + Unpin>(
    mut socket: S,
    transport: Arc<dyn Transport>,
    expected: &str,
    policy: ProxyPolicy,
    stats: &ProxyStats,
) -> io::Result<()> {
    let head = read_head(&mut socket).await?;
    if !head
        .auth
        .as_deref()
        .is_some_and(|given| constant_time_eq(given, expected))
    {
        reply(
            &mut socket,
            "407 Proxy Authentication Required",
            "proxy-authenticate: Basic realm=\"nrb\"\r\n",
        )
        .await;
        return Ok(());
    }
    if !head.method.eq_ignore_ascii_case("CONNECT") {
        reply(&mut socket, "405 Method Not Allowed", "allow: CONNECT\r\n").await;
        return Ok(());
    }
    let Some((host, port)) = split_host_port(&head.target) else {
        reply(&mut socket, "400 Bad Request", "").await;
        return Ok(());
    };
    if !policy.allow_private && is_private_target(&host) {
        reply(&mut socket, "403 Forbidden", "").await;
        stats.failed.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }

    stats.total.fetch_add(1, Ordering::Relaxed);
    let mut upstream = match transport.open_tcp(&host, port).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(%host, port, %error, "прокси: не удалось открыть поток");
            stats.failed.fetch_add(1, Ordering::Relaxed);
            reply(&mut socket, "502 Bad Gateway", "").await;
            return Ok(());
        }
    };
    socket
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    if !head.leftover.is_empty() {
        upstream.write_all(&head.leftover).await?;
    }

    stats.active.fetch_add(1, Ordering::Relaxed);
    // Счётчики обновляются по ходу передачи (а не по закрытию), чтобы индикатор в UI показывал живой трафик.
    let mut counted = Counted {
        inner: socket,
        stats,
    };
    let _ = tokio::io::copy_bidirectional(&mut counted, &mut upstream).await;
    stats.active.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn private_targets_are_detected() {
        for host in [
            "localhost",
            "foo.localhost",
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.5",
            "172.16.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "[::1]",
            "fd00::1",
            "fe80::1",
            "printer.local",
            "db.internal",
        ] {
            assert!(is_private_target(host), "{host} должен считаться приватным");
        }
        for host in [
            "example.org",
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700::1111",
            "100.63.0.1",
            "172.32.0.1",
        ] {
            assert!(
                !is_private_target(host),
                "{host} не должен считаться приватным"
            );
        }
    }

    #[test]
    fn host_port_parsing() {
        assert_eq!(
            split_host_port("example.org:443"),
            Some(("example.org".into(), 443))
        );
        assert_eq!(split_host_port("[::1]:8080"), Some(("::1".into(), 8080)));
        assert_eq!(split_host_port("example.org"), None);
        assert_eq!(split_host_port("example.org:0"), None);
        assert_eq!(split_host_port(":443"), None);
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
