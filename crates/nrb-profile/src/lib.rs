//! Профиль браузера: история и закладки (SQLite), сессия вкладок (JSON), настройки (TOML).
//! Куки, localStorage и IndexedDB хранит сам движок в том же каталоге (см. `nrb-engine-servo`).

mod settings;
mod store;

pub use settings::{SEARCH_ENGINES, Settings, Theme};
pub use store::{Bookmark, HistoryEntry, PermissionRecord, SessionState, Store, TabState};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("база профиля: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("файл профиля: {0}")]
    Io(#[from] std::io::Error),
    #[error("формат файла профиля: {0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, Error>;
