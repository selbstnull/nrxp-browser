//! `NodeManager` для окна «Узлы» (I7.1): спаренные узлы, выбор узла для туннеля, собственный узел в процессе браузера.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use nrb_node::{DeviceStore, IngressHandle, NodeFile, Role, run_ingress};
use nrb_shell::{DeviceRow, NodeManager, OwnNode, PairedNode};
use nrb_transport::NodeConfig;
use tokio::runtime::Runtime;

use crate::node;

pub struct Manager {
    node_dir: PathBuf,
    cli_override: bool,
    runtime: Runtime,
    ingress: Mutex<Option<IngressHandle>>,
    restart: AtomicBool,
}

impl Manager {
    pub fn new(cli_override: bool) -> Result<Self, String> {
        let node_dir = node::default_data_dir()
            .map(|d| d.join("node"))
            .ok_or("не удалось определить каталог данных")?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nrb-own-node")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            node_dir,
            cli_override,
            runtime,
            ingress: Mutex::new(None),
            restart: AtomicBool::new(false),
        })
    }

    /// Выполняет будущее на потоках рантайма менеджера (они в сети хоста), а не на вызывающем: `block_on` исполнял бы его
    /// на потоке окна, который в герметичном режиме живёт в пустом сетевом пространстве имён (узел «слушал» бы там).
    fn run<T: Send + 'static>(
        &self,
        future: impl std::future::Future<Output = T> + Send + 'static,
    ) -> T {
        self.runtime
            .block_on(self.runtime.spawn(future))
            .expect("задача менеджера узлов завершилась аварийно")
    }

    pub fn restart_requested(&self) -> bool {
        self.restart.load(Ordering::SeqCst)
    }

    /// Останавливает собственный узел (перед выходом/перезапуском).
    pub fn shutdown(&self) {
        if let Some(handle) = self.ingress.lock().expect("mutex ingress").take() {
            self.run(handle.shutdown());
        }
    }
}

impl NodeManager for Manager {
    fn paired(&self) -> Vec<PairedNode> {
        let Some(dir) = node::nodes_dir() else {
            return Vec::new();
        };
        let selected = node::selected_config();
        let mut nodes: Vec<PairedNode> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    return None;
                }
                let config = NodeConfig::from_toml_file(&path).ok()?;
                Some(PairedNode {
                    name: path.file_stem()?.to_string_lossy().into_owned(),
                    addr: config.addr,
                    selected: selected.as_deref() == Some(path.as_path()),
                })
            })
            .collect();
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        nodes
    }

    fn pair(&self, link: &str) -> Result<String, String> {
        if link.is_empty() {
            return Err("вставьте ссылку-приглашение".into());
        }
        node::pair_to_file(link, None).map(|(_, invite)| invite.name)
    }

    fn select(&self, name: Option<&str>) -> Result<(), String> {
        node::write_selection(name).map_err(|e| format!("не удалось сохранить выбор: {e}"))
    }

    fn cli_override(&self) -> bool {
        self.cli_override
    }

    fn own(&self) -> OwnNode {
        let Ok(file) = NodeFile::load(&self.node_dir) else {
            return OwnNode::default();
        };
        let devices = DeviceStore::open(&self.node_dir)
            .and_then(|s| s.list())
            .unwrap_or_default()
            .into_iter()
            .map(|d| DeviceRow {
                name: d.name,
                id: d.id,
                egress: d.role == Role::Egress,
                revoked: d.revoked,
            })
            .collect();
        let guard = self.ingress.lock().expect("mutex ingress");
        OwnNode {
            exists: true,
            running: guard.is_some(),
            listen: guard.as_ref().map_or_else(
                || format!("{}:{}", file.listen, file.port),
                |h| h.local_addr.to_string(),
            ),
            public_host: format!("{}:{}", file.public_host, file.port),
            public_key: file.public_key().unwrap_or_default(),
            exit: file.exit.clone(),
            devices,
            live_egress: guard.as_ref().map(|h| h.live_egress()).unwrap_or_default(),
            sessions: guard.as_ref().map_or(0, |h| h.active_sessions()),
        }
    }

    fn init_own(&self, public_host: &str, port: u16) -> Result<(), String> {
        if public_host.is_empty() {
            return Err("укажите внешний адрес узла".into());
        }
        NodeFile::init(&self.node_dir, public_host, port)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn set_ingress(&self, on: bool) -> Result<(), String> {
        let mut guard = self.ingress.lock().expect("mutex ingress");
        if on {
            if guard.is_some() {
                return Ok(());
            }
            let file = NodeFile::load(&self.node_dir).map_err(|e| e.to_string())?;
            let store =
                std::sync::Arc::new(DeviceStore::open(&self.node_dir).map_err(|e| e.to_string())?);
            let handle = self
                .run(async move { run_ingress(&file, store).await })
                .map_err(|e| e.to_string())?;
            *guard = Some(handle);
        } else if let Some(handle) = guard.take() {
            self.run(handle.shutdown());
        }
        Ok(())
    }

    fn invite(&self, device: &str, egress: bool) -> Result<String, String> {
        if device.is_empty() {
            return Err("укажите имя устройства".into());
        }
        node::make_invite(&self.node_dir, device, egress)
    }

    fn revoke(&self, device: &str) -> Result<(), String> {
        DeviceStore::open(&self.node_dir)
            .and_then(|s| s.revoke(device))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn request_restart(&self) {
        self.restart.store(true, Ordering::SeqCst);
    }
}
