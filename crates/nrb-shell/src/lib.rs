//! Оболочка браузера (I2): окно winit, вкладки и тулбар на egui поверх Servo.
//!
//! Схема кадра (как в servoshell): Servo рисует страницу в `OffscreenRenderingContext`, egui рисует тулбар прямо
//! в оконный GL-контекст, затем содержимое страницы блитится в нижнюю часть окна.

mod app;
mod downloads;
mod find;
mod keys;
mod library;
mod modal;
mod nodes;
mod passwords;
mod print;
mod site_data;
mod ui;

pub use app::{ShellConfig, TunnelStatus, run};
pub use downloads::{
    BodyEvent, DownloadHandler, DownloadRow, IncomingDownload, RowState, looks_like_download,
};
pub use nodes::{DeviceRow, NodeManager, OwnNode, PairedNode};

#[cfg(target_os = "android")]
pub use app::run_android;
/// Тип `AndroidApp` из android-activity (через winit): его получает `android_main`.
#[cfg(target_os = "android")]
pub use winit::platform::android::activity::AndroidApp;
