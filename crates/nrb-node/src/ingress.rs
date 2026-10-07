//! Приём клиентов NRXP (ingress): TCP-листенер + UDP datagram-нога, `ServerHandler` ядра на каждое соединение,
//! stealth-fallback для сканеров (решает ядро: «не наши» соединения прозрачно уходят на `decoy_host`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use netrunner_core::Identity;
use netrunner_core::net::{
    AuthValidator, Connection, NetworkConfig, ServerHandler, SessionManager, TunnelHandler,
    run_datagram_listener,
};
use tokio::net::{TcpListener, TcpSocket, UdpSocket};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::auth::{DeviceStore, LocalAuth};
use crate::{Error, NodeFile, Result};

/// Как часто проверять, не отозвали ли устройство с живой сессией.
const REVOKE_SWEEP: Duration = Duration::from_secs(2);

/// Работающий ingress. `shutdown` останавливает приём и ждёт завершения задачи приёма.
pub struct IngressHandle {
    pub local_addr: SocketAddr,
    token: CancellationToken,
    task: JoinHandle<()>,
    sessions: Arc<SessionManager>,
    auth: Arc<LocalAuth>,
}

impl IngressHandle {
    /// Сколько клиентских сессий сейчас на узле.
    pub fn active_sessions(&self) -> usize {
        self.sessions.get_session().len()
    }

    /// Трафик по устройствам (id → байты) с момента старта.
    pub fn usage(&self) -> std::collections::HashMap<String, u64> {
        self.auth.usage()
    }

    pub async fn shutdown(self) {
        self.token.cancel();
        let _ = self.task.await;
    }
}

fn init_core() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| NetworkConfig::init_global(1450));
}

async fn bind_tcp(addr: &str) -> std::io::Result<TcpListener> {
    let sock_addr = tokio::net::lookup_host(addr)
        .await?
        .next()
        .ok_or_else(|| std::io::Error::other("нет адреса для прослушивания"))?;
    let socket = if sock_addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
    // Как у netrunner-server: большой буфер приёма до listen (потолок буферов ног).
    let _ = socket.set_recv_buffer_size(netrunner_core::net::BUF_CAP as u32);
    socket.bind(sock_addr)?;
    socket.listen(1024)
}

/// Поднимает ingress узла. Порт `0` в `node.port` — эфемерный (для тестов); реальный адрес — в `local_addr`.
pub async fn run_ingress(node: &NodeFile, store: Arc<DeviceStore>) -> Result<IngressHandle> {
    init_core();
    let identity = Identity::Local(node.identity()?);
    let listener = bind_tcp(&format!("{}:{}", node.listen, node.port))
        .await
        .map_err(|e| {
            Error::Node(format!(
                "не удалось занять {}:{}: {e}",
                node.listen, node.port
            ))
        })?;
    let local_addr = listener.local_addr()?;

    let sessions = Arc::new(SessionManager::new());
    let auth = Arc::new(LocalAuth::new(store.clone()));
    let validator: Arc<dyn AuthValidator> = auth.clone();
    let token = CancellationToken::new();

    // UDP-нога на том же адресе и порту: опциональна, при неудаче клиенты остаются на TCP (как в netrunner-server).
    match UdpSocket::bind(local_addr).await {
        Ok(udp) => {
            let (sm, t) = (sessions.clone(), token.clone());
            tokio::spawn(async move {
                if let Err(error) = run_datagram_listener(udp, sm, t).await {
                    tracing::warn!(%error, "UDP-нога остановилась");
                }
            });
        }
        Err(error) => tracing::warn!(%error, "UDP-нога не поднялась, работаем только по TCP"),
    }

    // Отзыв: закрываем живые сессии отозванных устройств (токен проверяется только при установке сессии).
    {
        let (sm, store, t) = (sessions.clone(), store.clone(), token.clone());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = t.cancelled() => return,
                    _ = tokio::time::sleep(REVOKE_SWEEP) => {}
                }
                let revoked = store.revoked_ids();
                if revoked.is_empty() {
                    continue;
                }
                let muxers: Vec<_> = sm
                    .get_session()
                    .iter()
                    .map(|entry| entry.value().clone())
                    .collect();
                for muxer in muxers {
                    if muxer
                        .quota_user_id()
                        .is_some_and(|id| revoked.contains(&id))
                    {
                        tracing::info!("устройство отозвано — закрываю его сессию");
                        muxer.shutdown();
                    }
                }
            }
        });
    }

    let cover_flight: Arc<[usize]> = netrunner_core::decoy::CoverFlight::node_default()
        .as_records()
        .to_vec()
        .into();
    let decoy_host: Arc<str> = Arc::from(node.decoy_host.as_str());
    let (accept_sm, accept_token) = (sessions.clone(), token.clone());
    let task = tokio::spawn(async move {
        let mut next_conn: u64 = 0;
        loop {
            tokio::select! {
                _ = accept_token.cancelled() => break,
                accepted = listener.accept() => {
                    let Ok((stream, _peer)) = accepted else { continue };
                    // Адрес клиента в журнал не пишем (приватность): только порядковый номер соединения.
                    next_conn += 1;
                    let handler = ServerHandler::new(
                        Connection::new(stream),
                        accept_sm.clone(),
                        decoy_host.clone(),
                        Some(validator.clone()),
                        Some(identity.clone()),
                        cover_flight.clone(),
                        true,
                    )
                    .with_mesh_policy(true, false, None);
                    let span = tracing::info_span!("client_conn", conn = next_conn);
                    tokio::spawn(
                        async move {
                            if let Err(error) = handler.run().await {
                                tracing::debug!(%error, "соединение завершилось с ошибкой");
                            }
                        }
                        .instrument(span),
                    );
                }
            }
        }
    });

    Ok(IngressHandle {
        local_addr,
        token,
        task,
        sessions,
        auth,
    })
}
