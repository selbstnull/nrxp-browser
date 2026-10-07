//! Запуск графической оболочки (I2).

#[cfg(not(feature = "servo"))]
pub fn run(_urls: Vec<String>, _proxy: Option<String>, _node: crate::tunnel::NodeArgs) -> i32 {
    eprintln!(
        "nrb собран без движка: пересоберите с `--features servo` (или используйте `nrb node`)"
    );
    64
}

#[cfg(feature = "servo")]
pub fn run(urls: Vec<String>, proxy: Option<String>, node: crate::tunnel::NodeArgs) -> i32 {
    use nrb_engine::omnibox;
    use nrb_engine_servo::ServoConfig;
    use nrb_shell::ShellConfig;

    let search = omnibox::DEFAULT_SEARCH;
    let initial_urls = urls
        .iter()
        .filter_map(|input| omnibox::resolve(input, search))
        .collect();
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
        },
        search: search.to_owned(),
        tunnel: tunnel
            .clone()
            .map(|t| t as std::sync::Arc<dyn nrb_shell::TunnelStatus>),
    };
    match nrb_shell::run(config) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error:#}");
            3
        }
    }
}
