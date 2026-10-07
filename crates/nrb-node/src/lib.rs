//! Узел NRXP внутри `nrb` (I7). Ingress собран на публичном API `netrunner-core` (`ServerHandler`, `SessionManager`,
//! `AuthValidator`) — отдельной серверной библиотеки не требуется.
//!
//! * [`NodeFile`] — идентичность узла и параметры прослушивания (`node.toml`, права 0600);
//! * [`DeviceStore`]/[`LocalAuth`] — реестр спаренных устройств: токены хранятся хешами, отзыв рвёт живые сессии;
//! * [`Invite`] — ссылка-приглашение `nrxp://host:port?…`, превращается в конфиг клиента;
//! * [`run_ingress`] — приём клиентов с decoy-fallback для сканеров.

mod auth;
mod egress;
mod files;
mod ingress;
mod invite;

pub use auth::{Device, DeviceStore, LocalAuth, Role};
pub use egress::{Egress, EgressOptions};
pub use files::NodeFile;
pub use ingress::{IngressHandle, run_ingress};
pub use invite::Invite;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("файл узла: {0}")]
    Io(#[from] std::io::Error),
    #[error("формат файла узла: {0}")]
    Format(String),
    #[error("устройство «{0}» не найдено")]
    NoSuchDevice(String),
    #[error("устройство «{0}» уже есть")]
    DeviceExists(String),
    #[error("приглашение: {0}")]
    Invite(String),
    #[error("узел: {0}")]
    Node(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// 32 случайных байта в hex (секреты, токены).
pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("системный генератор случайных чисел");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Файл с секретами: создаётся с правами 0600 (на unix) атомарно через временный файл.
pub(crate) fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(tmp, path)
}
