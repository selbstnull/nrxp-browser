//! Запуск графической оболочки (I2).

#[cfg(not(feature = "servo"))]
pub fn run(_urls: Vec<String>, _proxy: Option<String>) -> i32 {
    eprintln!(
        "nrb собран без движка: пересоберите с `--features servo` (или используйте `nrb node`)"
    );
    64
}

#[cfg(feature = "servo")]
pub fn run(urls: Vec<String>, proxy: Option<String>) -> i32 {
    use nrb_engine::omnibox;
    use nrb_engine_servo::ServoConfig;
    use nrb_shell::ShellConfig;

    let search = omnibox::DEFAULT_SEARCH;
    let initial_urls = urls
        .iter()
        .filter_map(|input| omnibox::resolve(input, search))
        .collect();
    let config = ShellConfig {
        initial_urls,
        servo: ServoConfig {
            proxy,
            user_agent: None,
        },
        search: search.to_owned(),
    };
    match nrb_shell::run(config) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error:#}");
            3
        }
    }
}
