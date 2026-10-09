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
mod ui;

pub use app::{ShellConfig, TunnelStatus, run};
pub use downloads::{DownloadHandler, DownloadRow, RowState, looks_like_download};
pub use nodes::{DeviceRow, NodeManager, OwnNode, PairedNode};
