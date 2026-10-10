//! Android-приложение: `android_main` для NativeActivity. Скелет первой итерации — прямая сеть (без туннеля NRXP), профиль во
//! внутреннем каталоге приложения. Что ещё не сделано — `docs/PLATFORMS.md`, раздел «Android».
#![cfg(target_os = "android")]

use nrb_engine_servo::ServoConfig;
use nrb_shell::AndroidApp;
use nrb_shell::ShellConfig;

/// Каталоги приложения для общего кода (`nrb::platform` читает эти переменные окружения).
fn export_app_dirs(app: &AndroidApp) -> Option<std::path::PathBuf> {
    let data = app.internal_data_path()?;
    // SAFETY: вызывается один раз в начале `android_main`, до создания потоков, которые читали бы окружение.
    unsafe { std::env::set_var("NRB_DATA_DIR", &data) };
    if let Some(external) = app.external_data_path() {
        unsafe { std::env::set_var("NRB_DOWNLOADS_DIR", external.join("Download")) };
    }
    Some(data)
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("nrb"),
    );
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    let Some(data) = export_app_dirs(&app) else {
        log::error!("нет внутреннего каталога приложения");
        return;
    };
    let profile = data.join("profile");
    let _ = std::fs::create_dir_all(&profile);
    let config = ShellConfig {
        initial_urls: vec!["https://duckduckgo.com/".parse().expect("адрес")],
        servo: ServoConfig {
            profile_dir: Some(profile),
            // WebRTC и герметичный режим на Android не нужны/не поддерживаются в скелете.
            webrtc_forbidden: true,
            ..ServoConfig::default()
        },
        ..ShellConfig::default()
    };
    if let Err(error) = nrb_shell::run_android(config, app) {
        log::error!("оболочка завершилась с ошибкой: {error:#}");
    }
}
