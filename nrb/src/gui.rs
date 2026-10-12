//! Запуск графической оболочки (I2).

#[cfg(not(feature = "servo"))]
pub fn run(
    _urls: Vec<String>,
    _proxy: Option<String>,
    _node: crate::tunnel::NodeArgs,
    _profile: crate::profile::ProfileArgs,
    _devtools: Option<u16>,
) -> i32 {
    eprintln!(
        "nrb собран без движка: пересоберите с `--features servo` (или используйте `nrb node`)"
    );
    64
}

#[cfg(feature = "servo")]
pub fn run(
    urls: Vec<String>,
    proxy: Option<String>,
    node: crate::tunnel::NodeArgs,
    profile: crate::profile::ProfileArgs,
    devtools: Option<u16>,
) -> i32 {
    use nrb_engine::omnibox;
    use nrb_engine_servo::ServoConfig;
    use nrb_shell::ShellConfig;

    let search = omnibox::DEFAULT_SEARCH;
    let initial_urls = urls
        .iter()
        .filter_map(|input| omnibox::resolve(input, search))
        .collect();
    // Узел, выбранный в окне «Узлы», действует, если в командной строке узел не задан.
    let cli_override = node.enabled();
    let mut node = node;
    if !node.enabled()
        && let Some(path) = crate::node::selected_config()
    {
        node.node = Some(path);
    }
    let manager = match crate::gui_nodes::Manager::new(cli_override) {
        Ok(manager) => Some(std::sync::Arc::new(manager)),
        Err(error) => {
            eprintln!("окно «Узлы» недоступно: {error}");
            None
        }
    };
    if node.hermetic && !node.enabled() {
        eprintln!(
            "герметичный режим требует узла (--node или выбор в окне «Узлы»): прямого выхода в сеть у движка не будет"
        );
        return 3;
    }
    let tunnel = if node.enabled() {
        match crate::tunnel::start(&node).and_then(|mut tunnel| {
            // Менеджер узлов уже создан (его рантайм остался в сети хоста); дальше потоки наследуют пустое пространство имён.
            tunnel.isolate(node.hermetic)?;
            Ok(tunnel)
        }) {
            Ok(tunnel) => Some(std::sync::Arc::new(tunnel)),
            Err(error) => {
                eprintln!("туннель: {error}");
                return 3;
            }
        }
    } else {
        None
    };
    // Узел задан — движок ходит через коннектор (Servo сокетов не открывает), прокси ему не нужен; иначе — внешний `--proxy`.
    let network_connector = tunnel.as_ref().map(|t| t.engine_connector());
    let proxy = if network_connector.is_some() {
        None
    } else {
        proxy
    };
    let filters = profile.filters_dir();
    let userscripts = match profile.userscripts_dir() {
        Some(dir) => match nrb_userscripts::load_dir(&dir) {
            Ok(items) => items
                .iter()
                .filter_map(|item| {
                    let js = item.wrap();
                    if js.is_none() {
                        eprintln!("{}: нет @match — не запускается нигде", item.name);
                    }
                    js
                })
                .collect(),
            Err(error) => {
                eprintln!("пользовательские скрипты: {error}");
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    // Загрузки файлов идут клиентом nrb-net по тому же транспорту, что и страницы (туннель, иначе прямо).
    let downloads: Option<std::sync::Arc<dyn nrb_shell::DownloadHandler>> = match &tunnel {
        Some(t) => Some(std::sync::Arc::new(
            crate::gui_downloads::Handler::with_transport(t.transport(), t.runtime_handle(), None),
        )),
        None => match crate::gui_downloads::Handler::direct() {
            Ok(handler) => Some(std::sync::Arc::new(handler)),
            Err(error) => {
                eprintln!("загрузки недоступны: {error}");
                None
            }
        },
    };
    let config = ShellConfig {
        initial_urls,
        servo: ServoConfig {
            proxy,
            network_connector,
            user_agent: std::env::var("NRB_USER_AGENT")
                .ok()
                .filter(|s| !s.is_empty()),
            profile_dir: profile.for_window(),
            devtools_port: devtools,
            user_scripts: Vec::new(),
            webrtc_proxy: tunnel.as_ref().map(|t| t.webrtc_proxy()),
            webrtc_forbidden: node.hermetic || nrb_engine_servo::multiprocess_enabled(),
        },
        search: search.to_owned(),
        userscripts,
        adblock: filters.map(|dir| match nrb_adblock::Blocker::from_dir(&dir) {
            Ok(blocker) => std::sync::Arc::new(blocker),
            Err(error) => {
                eprintln!("списки фильтров {}: {error}", dir.display());
                std::sync::Arc::new(nrb_adblock::Blocker::empty())
            }
        }),
        tunnel: tunnel
            .clone()
            .map(|t| t as std::sync::Arc<dyn nrb_shell::TunnelStatus>),
        nodes: manager
            .clone()
            .map(|m| m as std::sync::Arc<dyn nrb_shell::NodeManager>),
        downloads,
    };
    let code = match nrb_shell::run(config) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error:#}");
            3
        }
    };
    nrb_engine_servo::cleanup_ephemeral();
    if let Some(manager) = manager {
        manager.shutdown();
        if manager.restart_requested() {
            // Новый экземпляр с теми же аргументами; выбранный узел он подхватит сам. Туннель закрываем до запуска.
            drop(tunnel);
            return relaunch(code);
        }
    }
    code
}

/// Запускает этот же исполняемый файл с теми же аргументами (после смены узла в окне «Узлы»).
#[cfg(feature = "servo")]
fn relaunch(code: i32) -> i32 {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("не удалось перезапуститься: {error}");
            return 3;
        }
    };
    match std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .spawn()
    {
        Ok(_) => code,
        Err(error) => {
            eprintln!("не удалось перезапуститься: {error}");
            3
        }
    }
}
