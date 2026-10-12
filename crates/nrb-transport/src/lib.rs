//! Транспорт браузера. Всё, что браузеру нужно от сети, — [`Transport::open_tcp`]; реализации:
//! [`SystemTransport`] (обычные сокеты — для тестов и разработки без узла) и [`NrxpTransport`]
//! (потоки NRXP напрямую через `netrunner-core`, без TUN/root/DNS ОС).

mod config;
mod nrxp;
mod system;

use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

pub use config::{ConfigError, NodeConfig};
pub use nrxp::{NrxpDatagrams, NrxpStream, NrxpTransport};
pub use system::SystemTransport;

/// Двунаправленный байтовый поток к цели.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}
pub type BoxStream = Box<dyn Stream>;

/// Датаграммный «сокет» к одной цели: границы сообщений сохраняются, доставка не гарантирована (как у UDP).
/// Основа для HTTP/3 (QUIC поверх него); вызывающий сам следит за потерями и повторами.
#[async_trait]
pub trait DatagramFlow: Send + Sync + 'static {
    /// Отправить одну датаграмму. Переполненная очередь туннеля — это потеря, а не ошибка (`Ok`), как у UDP;
    /// ошибка означает, что поток закрыт.
    fn send(&self, payload: &[u8]) -> io::Result<()>;
    /// Дождаться следующей входящей датаграммы; `Err(BrokenPipe)` — поток закрыт.
    async fn recv(&self) -> io::Result<bytes::Bytes>;
    /// Наибольшая датаграмма, которую можно отправить одним кадром.
    fn max_payload(&self) -> usize;
}
pub type BoxDatagramFlow = Box<dyn DatagramFlow>;

/// Что умеет текущий транспорт; от этого зависит выбор протокола (h3 только при `native_datagram_leg`, см. ARCHITECTURE §4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    /// Есть живая нативная datagram-нога (UDP-кадры не едут по TCP-ногам).
    pub native_datagram_leg: bool,
}

#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Открыть TCP-поток к `host:port`. Имя хоста передаётся как есть: резолвит выходная сторона, а не мы.
    async fn open_tcp(&self, host: &str, port: u16) -> io::Result<BoxStream>;
    /// Открыть датаграммный поток к `host:port` (UDP). Имя хоста, как и в TCP, резолвит выходная сторона.
    /// По умолчанию не поддерживается — вызывающий остаётся на TCP (HTTP/1.1, HTTP/2).
    async fn open_udp(&self, _host: &str, _port: u16) -> io::Result<BoxDatagramFlow> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "транспорт не умеет датаграммы",
        ))
    }
    fn caps(&self) -> Caps;
    fn name(&self) -> &'static str;
}
