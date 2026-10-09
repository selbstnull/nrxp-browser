//! Сетевой коннектор движка (N2 форка nrxp-servo): все исходящие TCP-соединения Servo (HTTP(S), WebSocket) открываются через
//! [`Transport`] — NRXP-поток до узла — без локального прокси и без единого сокета самого движка.
//!
//! Имя хоста передаётся транспорту как есть: резолвит выходная сторона, локально DNS не запрашивается. Цели
//! loopback/private/link-local по умолчанию запрещены (иначе страница сканировала бы сеть выходного узла) — то же правило, что у
//! `nrb_net::proxy::LocalProxy`.

use std::io;
use std::sync::Arc;

use nrb_net::proxy::{CountedStream, ProxyStats, is_private_target};
use nrb_transport::Transport;
use servo::{ConnectFuture, NetworkConnector, NetworkStream};

pub struct TransportConnector {
    transport: Arc<dyn Transport>,
    /// Рантайм туннеля: задачи потока NRXP (кредиты, сторож ног) живут в нём, а не в сетевом рантайме Servo.
    runtime: tokio::runtime::Handle,
    allow_private: bool,
    stats: Arc<ProxyStats>,
}

impl TransportConnector {
    /// `stats` — общие счётчики (их же показывает индикатор туннеля и считает локальный прокси).
    pub fn new(
        transport: Arc<dyn Transport>,
        runtime: tokio::runtime::Handle,
        allow_private: bool,
        stats: Arc<ProxyStats>,
    ) -> Self {
        Self {
            transport,
            runtime,
            allow_private,
            stats,
        }
    }

    /// Счётчики для индикатора в интерфейсе.
    pub fn stats(&self) -> &Arc<ProxyStats> {
        &self.stats
    }
}

impl NetworkConnector for TransportConnector {
    fn connect(&self, host: &str, port: u16) -> ConnectFuture {
        if !self.allow_private && is_private_target(host) {
            self.stats
                .failed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Box::pin(std::future::ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "цель во внутренней сети запрещена политикой (--allow-private)",
            ))));
        }
        let (transport, stats, host) = (self.transport.clone(), self.stats.clone(), host.to_owned());
        let handle = self.runtime.clone();
        Box::pin(async move {
            let failed_stats = stats.clone();
            let opened = handle
                .spawn(async move { transport.open_tcp(&host, port).await })
                .await
                .map_err(|e| io::Error::other(format!("задача туннеля прервана: {e}")))?;
            match opened {
                Ok(stream) => Ok(Box::new(CountedStream::new(stream, stats)) as Box<dyn NetworkStream>),
                Err(error) => {
                    failed_stats
                        .failed
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Err(error)
                }
            }
        })
    }
}
