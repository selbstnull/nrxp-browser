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
    let tunnel = if node.enabled() {
        match crate::tunnel::start(&node) {
            Ok(tunnel) => Some(std::sync::Arc::new(tunnel)),
            Err(error) => {
                eprintln!("туннель: {error}");
                return 3;
            }
        }
    } else {
        None
    };
    let proxy = tunnel.as_ref().map(|t| t.proxy_url()).or(proxy);
    let config = ShellConfig {
        initial_urls,
        servo: ServoConfig {
            proxy,
            user_agent: None,
            profile_dir: profile.for_window(),
            devtools_port: devtools,
        },
        search: search.to_owned(),
        tunnel: tunnel
            .clone()
            .map(|t| t as std::sync::Arc<dyn nrb_shell::TunnelStatus>),
        nodes: manager
            .clone()
            .map(|m| m as std::sync::Arc<dyn nrb_shell::NodeManager>),
    };
    let code = match nrb_shell::run(config) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error:#}");
            3
        }
    };
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
