//! `nrb node …` и `nrb pair` (I7): свой узел NRXP без бэкенда — создание, запуск, приглашения, отзыв устройств.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, Subcommand};
use nrb_node::{DeviceStore, Egress, EgressOptions, Invite, NodeFile, Role, run_ingress};

#[derive(Args, Clone, Debug)]
pub struct DirArg {
    /// Каталог узла (node.toml с секретами, devices.json). По умолчанию `~/.local/share/netrunner-browser/node`.
    #[arg(long, env = "NRB_NODE_DIR")]
    pub dir: Option<PathBuf>,
}

impl DirArg {
    fn resolve(&self) -> Result<PathBuf, String> {
        self.dir
            .clone()
            .or_else(default_node_dir)
            .ok_or_else(|| "не удалось определить каталог узла: задайте --dir".to_owned())
    }
}

#[derive(Subcommand)]
pub enum NodeCommand {
    /// Создать новый узел: свежие секрет и ключ (существующий не затирается).
    Init {
        #[command(flatten)]
        dir: DirArg,
        /// Внешний адрес узла (IP или домен) — он попадёт в приглашения.
        #[arg(long)]
        public_host: String,
        #[arg(long, default_value_t = 443)]
        port: u16,
    },
    /// Запустить узел (ingress). Останавливается по Ctrl+C / SIGTERM.
    Run {
        #[command(flatten)]
        dir: DirArg,
    },
    /// Выдать приглашение для нового устройства (токен показывается один раз).
    Invite {
        #[command(flatten)]
        dir: DirArg,
        /// Имя устройства, например `телефон`.
        name: String,
        /// Приглашение для домашнего выхода (reverse-egress): токен годится только для `nrb egress`, не как клиентский.
        #[arg(long)]
        egress: bool,
    },
    /// Список устройств.
    Devices {
        #[command(flatten)]
        dir: DirArg,
    },
    /// Выход трафика устройства: `local` (с самого узла), метка домашнего выхода или `default` (по режиму узла).
    Exit {
        #[command(flatten)]
        dir: DirArg,
        /// Имя устройства.
        device: String,
        /// `local` | `default` | метка выхода (имя из `nrb egress --label`).
        via: String,
    },
    /// Режим выхода клиентов узла: `local`, `prefer` (через домашний выход, если подключён) или `require` (только через него).
    Mode {
        #[command(flatten)]
        dir: DirArg,
        mode: String,
    },
    /// Отозвать устройство: токен перестаёт работать, живая сессия закрывается.
    Revoke {
        #[command(flatten)]
        dir: DirArg,
        name: String,
    },
}

pub fn default_data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("netrunner-browser"))
}

fn default_node_dir() -> Option<PathBuf> {
    default_data_dir().map(|d| d.join("node"))
}

pub fn run(command: NodeCommand) -> i32 {
    match execute(command) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn execute(command: NodeCommand) -> Result<(), String> {
    match command {
        NodeCommand::Init {
            dir,
            public_host,
            port,
        } => {
            let dir = dir.resolve()?;
            let node = NodeFile::init(&dir, &public_host, port).map_err(|e| e.to_string())?;
            println!("узел создан в {}", dir.display());
            println!(
                "  публичный ключ: {}",
                node.public_key().map_err(|e| e.to_string())?
            );
            println!(
                "  слушает: {}:{}   в приглашениях: {public_host}:{port}",
                node.listen, node.port
            );
            println!("  decoy-домен: {}", node.decoy_host);
            println!("Дальше: `nrb node invite <имя устройства>` и `nrb node run`.");
            println!(
                "Файл node.toml содержит СЕКРЕТЫ (права 0600) — не публиковать и не коммитить."
            );
            Ok(())
        }
        NodeCommand::Invite { dir, name, egress } => {
            let dir = dir.resolve()?;
            println!("{}", make_invite(&dir, &name, egress)?);
            eprintln!(
                "\nПриглашение содержит секреты — передавайте по доверенному каналу. Токен показан ОДИН раз (на узле хранится только хеш)."
            );
            if egress {
                eprintln!("На домашнем устройстве: `nrb egress '<ссылка>'`.");
                eprintln!(
                    "Чтобы клиенты узла выходили через него, в node.toml задайте exit = \"prefer\" или \"require\" и перезапустите узел."
                );
            } else {
                eprintln!(
                    "На новом устройстве: `nrb pair '<ссылка>'` или `nrb --node '<ссылка>' <url>`."
                );
            }
            Ok(())
        }
        NodeCommand::Devices { dir } => {
            let store = DeviceStore::open(&dir.resolve()?).map_err(|e| e.to_string())?;
            let devices = store.list().map_err(|e| e.to_string())?;
            if devices.is_empty() {
                println!("Устройств нет. `nrb node invite <имя>` — добавить.");
            }
            for device in devices {
                println!(
                    "{}  {:<24} {:<8} {:<10} {}",
                    device.id,
                    device.name,
                    match device.role {
                        Role::Client => "клиент",
                        Role::Egress => "выход",
                    },
                    match (device.role, device.exit_via.as_deref()) {
                        (Role::Client, Some(via)) => format!("→ {via}"),
                        _ => String::new(),
                    },
                    if device.revoked {
                        "ОТОЗВАНО"
                    } else {
                        "активно"
                    }
                );
            }
            Ok(())
        }
        NodeCommand::Exit { dir, device, via } => {
            let store = DeviceStore::open(&dir.resolve()?).map_err(|e| e.to_string())?;
            let value = match via.as_str() {
                "default" | "" => None,
                other => Some(other.to_owned()),
            };
            store
                .set_exit_via(&device, value.clone())
                .map_err(|e| e.to_string())?;
            println!(
                "«{device}»: выход — {}. Работающий узел подхватит изменение сам.",
                value.as_deref().unwrap_or("по режиму узла")
            );
            Ok(())
        }
        NodeCommand::Mode { dir, mode } => {
            let dir = dir.resolve()?;
            let mut node = NodeFile::load(&dir).map_err(|e| e.to_string())?;
            let previous = std::mem::replace(&mut node.exit, mode.clone());
            if let Err(error) = node.exit_mode() {
                return Err(format!("{error}"));
            }
            node.save(&dir).map_err(|e| e.to_string())?;
            println!(
                "режим выхода: {previous} → {mode}. Работающий узел подхватит его в течение нескольких секунд."
            );
            Ok(())
        }
        NodeCommand::Revoke { dir, name } => {
            let store = DeviceStore::open(&dir.resolve()?).map_err(|e| e.to_string())?;
            store.revoke(&name).map_err(|e| e.to_string())?;
            println!(
                "«{name}» отозвано. Работающий узел закроет его сессию в течение нескольких секунд."
            );
            Ok(())
        }
        NodeCommand::Run { dir } => run_node(&dir.resolve()?),
    }
}

/// Новое устройство в реестре узла и ссылка-приглашение для него (токен — один раз).
pub fn make_invite(dir: &Path, name: &str, egress: bool) -> Result<String, String> {
    let node = NodeFile::load(dir).map_err(|e| e.to_string())?;
    let store = DeviceStore::open(dir).map_err(|e| e.to_string())?;
    let role = if egress { Role::Egress } else { Role::Client };
    let (_, token) = store
        .add_device_with_role(name, role)
        .map_err(|e| e.to_string())?;
    let invite = Invite {
        host: node.public_host.clone(),
        port: node.port,
        public_key: node.public_key().map_err(|e| e.to_string())?,
        secret: node.secret.clone(),
        token,
        decoy_sni: node.decoy_host.clone(),
        name: name.to_owned(),
    };
    Ok(invite.to_url())
}

fn run_node(dir: &Path) -> Result<(), String> {
    let node = NodeFile::load(dir).map_err(|e| e.to_string())?;
    let store = Arc::new(DeviceStore::open(dir).map_err(|e| e.to_string())?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let ingress = run_ingress(&node, store.clone())
            .await
            .map_err(|e| e.to_string())?;
        println!(
            "узел слушает {} (TCP+UDP), decoy: {}",
            ingress.local_addr, node.decoy_host
        );
        println!(
            "публичный ключ: {}",
            node.public_key().map_err(|e| e.to_string())?
        );
        println!(
            "выход клиентов: {} (node.toml: exit)",
            match node.exit.as_str() {
                "prefer" => "через домашний egress, если подключён, иначе с узла",
                "require" => "ТОЛЬКО через домашний egress (без него — отказ)",
                _ => "с самого узла",
            }
        );
        println!(
            "устройств: {}. Остановка — Ctrl+C.",
            store
                .list()
                .map(|d| d.iter().filter(|d| !d.revoked).count())
                .unwrap_or(0)
        );
        wait_for_shutdown().await;
        println!("останавливаюсь…");
        ingress.shutdown().await;
        Ok(())
    })
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("обработчик SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// `nrb egress <ссылка|конфиг>`: домашний выход. Держит сессию с ingress до Ctrl+C или отзыва токена.
pub fn egress(
    node: &str,
    label: Option<String>,
    allow_lan: bool,
    max_streams: Option<usize>,
) -> i32 {
    let (config, default_label) = if node.starts_with("nrxp://") {
        match Invite::parse(node) {
            Ok(invite) => {
                let name = invite.name.clone();
                (invite.to_node_config(), name)
            }
            Err(error) => {
                eprintln!("приглашение: {error}");
                return 1;
            }
        }
    } else {
        match nrb_transport::NodeConfig::from_toml_file(Path::new(node)) {
            Ok(config) => (config, "home".to_owned()),
            Err(error) => {
                eprintln!("{node}: {error}");
                return 1;
            }
        }
    };
    let options = EgressOptions {
        label: label.unwrap_or(default_label),
        allow_private: allow_lan,
        max_streams,
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    runtime.block_on(async {
        let egress =
            match Egress::start(&config, &options, std::time::Duration::from_secs(20)).await {
                Ok(egress) => egress,
                Err(error) => {
                    eprintln!("не удалось подключиться к узлу {}: {error}", config.addr);
                    return 1;
                }
            };
        println!(
            "выход «{}» подключён к {}. Целей в локальной сети: {}. Остановка — Ctrl+C.",
            options.label,
            config.addr,
            if allow_lan {
                "РАЗРЕШЕНЫ"
            } else {
                "запрещены"
            }
        );
        let mut was_up = true;
        let shutdown = wait_for_shutdown();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            }
            if egress.rejected() {
                eprintln!("узел отверг или отозвал токен выхода — завершаюсь");
                return 1;
            }
            let up = egress.up();
            if up != was_up {
                println!(
                    "{}",
                    if up {
                        "связь с узлом восстановлена"
                    } else {
                        "связи с узлом нет, переподключаюсь…"
                    }
                );
                was_up = up;
            }
        }
        println!("останавливаюсь…");
        0
    })
}

/// `nrb pair <ссылка>`: сохраняет конфиг узла для этого устройства.
pub fn pair(link: &str, out: Option<PathBuf>) -> i32 {
    let (path, invite) = match pair_to_file(link, out) {
        Ok(done) => done,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    println!(
        "спарено с {}:{} как «{}»",
        invite.host, invite.port, invite.name
    );
    println!("конфиг (с секретами, права 0600): {}", path.display());
    println!("Использование: nrb --node {} <адрес>", path.display());
    0
}

/// Разбирает приглашение и сохраняет конфиг узла (0600). Ошибки не содержат ссылку: в ней секреты.
pub fn pair_to_file(link: &str, out: Option<PathBuf>) -> Result<(PathBuf, Invite), String> {
    let invite = Invite::parse(link).map_err(|e| e.to_string())?;
    let path = out
        .or_else(|| nodes_dir().map(|d| d.join(format!("{}.toml", sanitize(&invite.name)))))
        .ok_or("не удалось определить, куда сохранить конфиг: задайте --out")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    write_private(&path, &invite.to_config_toml())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path, invite))
}

/// Каталог конфигов спаренных узлов.
pub fn nodes_dir() -> Option<PathBuf> {
    default_data_dir().map(|d| d.join("nodes"))
}

/// Файл с выбором узла для туннеля (имя конфига без `.toml`); нет файла — напрямую.
#[cfg_attr(not(feature = "servo"), allow(dead_code))]
fn selection_file() -> Option<PathBuf> {
    default_data_dir().map(|d| d.join("selected-node"))
}

/// Конфиг выбранного в окне «Узлы» узла, если он есть и ещё существует.
#[cfg_attr(not(feature = "servo"), allow(dead_code))]
pub fn selected_config() -> Option<PathBuf> {
    let name = std::fs::read_to_string(selection_file()?).ok()?;
    let name = name.trim();
    if name.is_empty() || name != sanitize(name) {
        return None;
    }
    let path = nodes_dir()?.join(format!("{name}.toml"));
    path.is_file().then_some(path)
}

#[cfg_attr(not(feature = "servo"), allow(dead_code))]
pub fn write_selection(name: Option<&str>) -> std::io::Result<()> {
    let Some(path) = selection_file() else {
        return Err(std::io::Error::other("нет каталога данных"));
    };
    match name {
        Some(name) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, sanitize(name))
        }
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "node".to_owned()
    } else {
        cleaned
    }
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn device_names_become_safe_file_names() {
        assert_eq!(sanitize("мой телефон"), "мой_телефон");
        assert_eq!(sanitize("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize(""), "node");
    }
}
