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
pub use nrxp::{NrxpStream, NrxpTransport};
pub use system::SystemTransport;

/// Двунаправленный байтовый поток к цели.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}
pub type BoxStream = Box<dyn Stream>;

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
    fn caps(&self) -> Caps;
    fn name(&self) -> &'static str;
}
