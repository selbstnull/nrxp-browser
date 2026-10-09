//! Туннель браузера (I5): сессия NRXP + локальный CONNECT-прокси, на который настраивается движок.
//! Весь трафик движка идёт `Servo → прокси (loopback, логин/пароль) → NrxpTransport → узел`.

use std::path::PathBuf;
use std::sync::Arc;

use nrb_net::proxy::{LocalProxy, ProxyPolicy};
use nrb_transport::{NodeConfig, NrxpTransport};
use tokio::runtime::Runtime;

#[derive(clap::Args, Clone, Debug, Default)]
pub struct NodeArgs {
    /// Конфиг узла NRXP (TOML: addr, decoy_sni, nrxp_secret, nrxp_public_key, auth_token). С ним весь трафик движка идёт через узел.
    #[arg(long, conflicts_with = "node_addr", env = "NRB_NODE_CONFIG")]
    pub node: Option<PathBuf>,
    /// Анонимный узел `host:port` (только для локальной разработки).
    #[arg(long)]
    pub node_addr: Option<String>,
    /// Разрешить через туннель цели loopback/private/link-local (по умолчанию запрещены: иначе страница сканирует сеть узла).
    #[arg(long)]
    pub allow_private: bool,
    /// Герметичный режим (Linux, I11): движок в пустом сетевом пространстве имён — прямых соединений у него нет по построению,
    /// наружу только через прокси туннеля. Требует узла; без пользовательских пространств имён отказывает, а не деградирует.
    #[arg(long, env = "NRB_HERMETIC")]
    pub hermetic: bool,
}

impl NodeArgs {
    pub fn enabled(&self) -> bool {
        self.node.is_some() || self.node_addr.is_some()
    }
}

#[cfg_attr(not(feature = "servo"), allow(dead_code))]
pub struct Tunnel {
    // Порядок важен: прокси и транспорт должны умереть раньше рантайма.
    proxy: LocalProxy,
    transport: Arc<NrxpTransport>,
    label: String,
    allow_private: bool,
    isolated: bool,
    runtime: Runtime,
}

pub fn start(args: &NodeArgs) -> Result<Tunnel, String> {
    let config = match (&args.node, &args.node_addr) {
        // `--node 'nrxp://…'` — приглашение прямо в аргументе (без предварительного `nrb pair`). В сообщениях об ошибках
        // саму ссылку не печатаем: в ней секреты узла и токен устройства.
        (Some(link), _) if link.to_string_lossy().starts_with("nrxp://") => {
            nrb_node::Invite::parse(&link.to_string_lossy())
                .map_err(|e| format!("приглашение: {e}"))?
                .to_node_config()
        }
        (Some(path), _) => {
            NodeConfig::from_toml_file(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        (None, Some(addr)) => NodeConfig::anonymous(addr.clone()),
        (None, None) => return Err("не задан узел (--node или --node-addr)".into()),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("nrb-tunnel")
        .enable_all()
        .build()
        .map_err(|e| format!("рантайм туннеля: {e}"))?;
    let label = config.addr.clone();
    let (transport, proxy) = runtime.block_on(async {
        let transport = Arc::new(
            NrxpTransport::connect(&config)
                .await
                .map_err(|e| format!("подключение к узлу {}: {e}", config.addr))?,
        );
        // Локальный CONNECT-прокси остаётся для дочерних процессов (`nrb corpus` раздаёт его снимкам через NRB_PROXY_URL);
        // движок в этом процессе ходит напрямую через коннектор (см. `engine_connector`).
        let proxy = LocalProxy::start(
            transport.clone(),
            ProxyPolicy {
                allow_private: args.allow_private,
            },
        )
        .await
        .map_err(|e| format!("локальный прокси: {e}"))?;
        Ok::<_, String>((transport, proxy))
    })?;
    Ok(Tunnel {
        proxy,
        transport,
        label,
        allow_private: args.allow_private,
        isolated: false,
        runtime,
    })
}

impl Tunnel {
    /// `http://логин:пароль@127.0.0.1:порт` — для дочерних процессов (`NRB_PROXY_URL`).
    pub fn proxy_url(&self) -> String {
        self.proxy.url()
    }

    /// Коннектор для движка: все TCP-соединения Servo идут в NRXP-поток, сокетов у движка нет.
    #[cfg(feature = "servo")]
    pub fn engine_connector(&self) -> nrb_engine_servo::ConnectorHandle {
        nrb_engine_servo::ConnectorHandle(Arc::new(nrb_engine_servo::TransportConnector::new(
            self.transport.clone(),
            self.runtime.handle().clone(),
            self.allow_private,
            self.proxy.stats().clone(),
        )))
    }

    /// Герметичный режим: входит **этим потоком** в пустое сетевое пространство имён. Звать после создания всех рантаймов с
    /// сетью (туннель, менеджер узлов) и перед созданием движка. Релея и прокси не нужно: движок ходит через коннектор, а
    /// поток NRXP живёт в рантайме туннеля (сеть хоста). Без `--hermetic` ничего не делает.
    #[cfg_attr(not(feature = "servo"), allow(dead_code))]
    pub fn isolate(&mut self, hermetic: bool) -> Result<(), String> {
        if !hermetic {
            return Ok(());
        }
        crate::hermetic::enter_empty_namespace()?;
        self.isolated = true;
        Ok(())
    }

    /// Рантайм туннеля (для проверок, которым нужно открыть поток так же, как это делает движок).
    #[cfg_attr(not(feature = "servo"), allow(dead_code))]
    pub fn transport(&self) -> Arc<NrxpTransport> {
        self.transport.clone()
    }

    #[cfg_attr(not(feature = "servo"), allow(dead_code))]
    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }
}

#[cfg(feature = "servo")]
impl nrb_shell::TunnelStatus for Tunnel {
    fn up(&self) -> bool {
        self.transport.muxer().active_legs_count() > 0
    }

    fn summary(&self) -> String {
        use std::sync::atomic::Ordering;
        let stats = self.proxy.stats();
        let legs = self.transport.muxer().active_legs_count();
        let human = |bytes: u64| match bytes {
            b if b >= 1 << 30 => format!("{:.1} ГБ", b as f64 / (1u64 << 30) as f64),
            b if b >= 1 << 20 => format!("{:.1} МБ", b as f64 / (1u64 << 20) as f64),
            b => format!("{:.0} КБ", b as f64 / 1024.0),
        };
        format!(
            "{}NRXP {} · ног {legs} · соед. {} · вх {} исх {}",
            if self.isolated {
                "герметично · "
            } else {
                ""
            },
            self.label,
            stats.active.load(Ordering::Relaxed),
            human(stats.bytes_down.load(Ordering::Relaxed)),
            human(stats.bytes_up.load(Ordering::Relaxed)),
        )
    }
}
