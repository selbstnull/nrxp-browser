//! Деплой узла `nrb` на VPS по SSH (I8).
//!
//! Логика написана поверх трейта [`Remote`] (выполнить команду, залить файл): настоящая реализация — [`ssh::SshRemote`] на
//! `russh` (системный `ssh` не нужен), в юнит-тестах — поддельная. Порядок шагов и откат описаны в [`deploy`].
//!
//! Принципы: секреты узла рождаются **на сервере** (наружу уходит только публичный ключ и приглашение); хост-ключ SSH
//! проверяется и пинится, молча не принимается; любой сбой после первого изменения откатывается; повторный запуск = апгрейд.

mod elf;
mod known_hosts;
mod remote;
mod runner;
mod ssh;
mod unit;
mod verify;

pub use elf::{BinaryArch, binary_arch};
pub use known_hosts::KnownHosts;
pub use remote::{ExecOutput, Remote, sh_quote};
pub use runner::{DeployOptions, DeployReport, Probe, Verifier, deploy, remove};
pub use ssh::{HostKeyPolicy, SshAuth, SshRemote};
pub use unit::{SERVICE_NAME, service_unit};
pub use verify::NrxpVerifier;

#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error("SSH: {0}")]
    Ssh(String),
    #[error("хост-ключ сервера: {0}")]
    HostKey(String),
    #[error("сервер не подходит: {0}")]
    Unsuitable(String),
    #[error("{step}: {detail}")]
    Step { step: &'static str, detail: String },
    #[error("локальный файл: {0}")]
    Local(String),
    #[error("проверка после установки не пройдена: {0}")]
    Verify(String),
}

pub type Result<T> = std::result::Result<T, DeployError>;
