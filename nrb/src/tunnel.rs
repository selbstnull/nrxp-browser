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
    // Порядок важен: релей и прокси и транспорт должны умереть раньше рантайма.
    isolation: Option<crate::hermetic::Isolated>,
    proxy: LocalProxy,
    transport: Arc<NrxpTransport>,
    label: String,
    hermetic_dir: Option<std::path::PathBuf>,
    _runtime: Runtime,
}

pub fn start(args: &NodeArgs) -> Result<Tunnel, String> {
    if args.hermetic {
        let dir = crate::hermetic::socket_dir().map_err(|e| format!("каталог сокета: {e}"))?;
        let mut tunnel = start_with(args, Some(&dir.join("proxy.sock")))?;
        tunnel.hermetic_dir = Some(dir);
        return Ok(tunnel);
    }
    start_with(args, None)
}

/// Герметичный вариант (I11): прокси слушает unix-сокет, TCP-порта на хосте нет.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn start_unix(args: &NodeArgs, socket: &std::path::Path) -> Result<Tunnel, String> {
    start_with(args, Some(socket))
}

fn start_with(args: &NodeArgs, unix_socket: Option<&std::path::Path>) -> Result<Tunnel, String> {
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
        let policy = ProxyPolicy {
            allow_private: args.allow_private,
        };
        let proxy = match unix_socket {
            #[cfg(unix)]
            Some(path) => LocalProxy::start_unix(path, transport.clone(), policy).await,
            #[cfg(not(unix))]
            Some(_) => Err(std::io::Error::other(
                "unix-сокеты недоступны на этой платформе",
            )),
            None => LocalProxy::start(transport.clone(), policy).await,
        }
        .map_err(|e| format!("локальный прокси: {e}"))?;
        Ok::<_, String>((transport, proxy))
    })?;
    Ok(Tunnel {
        isolation: None,
        hermetic_dir: None,
        proxy,
        transport,
        label,
        _runtime: runtime,
    })
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Some(dir) = &self.hermetic_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Tunnel {
    /// Логин и пароль прокси (для герметичного режима URL собирается с портом релея).
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn credentials(&self) -> (&str, &str) {
        self.proxy.credentials()
    }

    /// `http://логин:пароль@127.0.0.1:порт` — для prefs движка и переменной `NRB_PROXY_URL` дочерних процессов.
    pub fn proxy_url(&self) -> String {
        match &self.isolation {
            Some(isolation) => isolation.url.clone(),
            None => self.proxy.url(),
        }
    }

    /// Герметичный режим: входит **этим потоком** в пустое сетевое пространство имён и поднимает релей. Звать после создания
    /// всех рантаймов с сетью (туннель, менеджер узлов) и перед созданием движка. Без `--hermetic` ничего не делает.
    #[cfg_attr(not(feature = "servo"), allow(dead_code))]
    pub fn isolate(&mut self) -> Result<(), String> {
        let Some(dir) = self.hermetic_dir.take() else {
            return Ok(());
        };
        let socket = dir.join("proxy.sock");
        let creds = self.proxy.credentials();
        let (relay, url) = crate::hermetic::isolate(&socket, creds)?;
        self.isolation = Some(crate::hermetic::Isolated {
            url,
            _relay: relay,
            dir,
        });
        Ok(())
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
            if self.isolation.is_some() {
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
