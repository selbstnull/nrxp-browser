//! Домашний `nrb` как выход (reverse-egress, I9): сам дозванивается до ingress на VPS и обслуживает потоки,
//! которые тот ему направляет. Входящих портов не нужно — работает за NAT.
//!
//! Что разрешено выходу — `EgressPolicy` ядра: по умолчанию loopback/private/link-local/CGNAT/metadata недоступны
//! (иначе страница в браузере клиента сканировала бы домашнюю сеть). `allow_private` включает их явно.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use netrunner_core::net::{ClientHandler, EgressPolicy, Muxer, NetworkConfig};
use nrb_transport::NodeConfig;

#[derive(Debug, Clone, Default)]
pub struct EgressOptions {
    /// Подпись выхода в реестре ingress (без секретов), например имя устройства.
    pub label: String,
    /// Разрешить цели в локальной сети этого устройства. По умолчанию нет.
    pub allow_private: bool,
    /// Максимум одновременных потоков, которые выход обслуживает; лишние отклоняются. `None` — без лимита.
    pub max_streams: Option<usize>,
}

/// Работающий выход. Пока объект жив, сессия держится и переподключается ядром.
pub struct Egress {
    muxer: Arc<Muxer>,
}

impl Egress {
    /// Подключается к ingress из конфига (`NodeConfig`, токен — роли `egress`) и ждёт первую ногу.
    pub async fn start(
        config: &NodeConfig,
        options: &EgressOptions,
        timeout: Duration,
    ) -> io::Result<Self> {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| NetworkConfig::init_global(1450));
        let identity = config
            .identity()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let muxer = ClientHandler::connect_egress(
            &config.addr,
            config.decoy_sni.clone(),
            config.auth_token.clone(),
            &options.label,
            identity,
            EgressPolicy {
                allow_private: options.allow_private,
                max_streams: options.max_streams,
            },
        )
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e.to_string()))?;
        let deadline = Instant::now() + timeout;
        while muxer.active_legs_count() == 0 {
            if muxer.is_fatal() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "узел отверг токен выхода (нужно приглашение `nrb node invite --egress`)",
                ));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "узел не ответил вовремя",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(Self { muxer })
    }

    pub fn up(&self) -> bool {
        !self.muxer.is_fatal() && self.muxer.active_legs_count() > 0
    }

    /// Сессия завершена навсегда (токен отозван/отвергнут) — переподключаться бессмысленно.
    pub fn rejected(&self) -> bool {
        self.muxer.is_fatal()
    }

    pub fn legs(&self) -> usize {
        self.muxer.active_legs_count()
    }
}

impl Drop for Egress {
    fn drop(&mut self) {
        self.muxer.shutdown();
    }
}
