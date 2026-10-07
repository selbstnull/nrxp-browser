use std::fmt;
use std::path::Path;
use std::time::Duration;

use netrunner_core::{Identity, PeerIdentity};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("не удалось прочитать конфиг узла: {0}")]
    Io(#[from] std::io::Error),
    #[error("некорректный TOML конфига узла: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("не хватает поля `{0}`")]
    Missing(&'static str),
    #[error("учётные данные узла: {0}")]
    Identity(String),
}

fn default_decoy() -> String {
    netrunner_core::net::DEFAULT_DECOY_HOST.to_owned()
}

fn default_connect_timeout() -> u64 {
    10
}

/// Как подключиться к узлу NRXP. Формат совместим с тем, что отдаёт backend (`/nodes/routing`):
/// адрес, decoy-SNI, секрет и статический публичный ключ узла.
#[derive(Clone, Deserialize)]
pub struct NodeConfig {
    /// `host:port` узла.
    pub addr: String,
    /// SNI, под который маскируется `ClientHello`; обязан совпадать с `--decoy-host` узла.
    #[serde(default = "default_decoy")]
    pub decoy_sni: String,
    /// Bearer-токен (если узел запущен с `--require-auth`).
    pub auth_token: Option<String>,
    /// `nrxp_secret` узла, hex 64 символа. Задаётся вместе с `nrxp_public_key`.
    pub nrxp_secret: Option<String>,
    /// `nrxp_static_public` узла, hex 64 символа.
    pub nrxp_public_key: Option<String>,
    /// Сколько секунд ждать появления первой ноги туннеля.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
}

impl fmt::Debug for NodeConfig {
    // Секреты в логи не попадают.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeConfig")
            .field("addr", &self.addr)
            .field("decoy_sni", &self.decoy_sni)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "nrxp_secret",
                &self.nrxp_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("nrxp_public_key", &self.nrxp_public_key)
            .finish()
    }
}

impl NodeConfig {
    /// Анонимный узел (схема v2, без учётных данных) — только для локальной разработки.
    pub fn anonymous(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            decoy_sni: default_decoy(),
            auth_token: None,
            nrxp_secret: None,
            nrxp_public_key: None,
            connect_timeout_secs: default_connect_timeout(),
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    pub fn from_toml_file(path: &Path) -> Result<Self, ConfigError> {
        Self::from_toml(&std::fs::read_to_string(path)?)
    }

    /// Из переменных окружения: `NRB_NODE_ADDR` (обязательна), `NRB_NODE_DECOY_SNI`, `NRB_NODE_AUTH_TOKEN`,
    /// `NRB_NODE_SECRET`, `NRB_NODE_PUBLIC_KEY`.
    pub fn from_env() -> Result<Self, ConfigError> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let mut config =
            Self::anonymous(var("NRB_NODE_ADDR").ok_or(ConfigError::Missing("NRB_NODE_ADDR"))?);
        if let Some(sni) = var("NRB_NODE_DECOY_SNI") {
            config.decoy_sni = sni;
        }
        config.auth_token = var("NRB_NODE_AUTH_TOKEN");
        config.nrxp_secret = var("NRB_NODE_SECRET");
        config.nrxp_public_key = var("NRB_NODE_PUBLIC_KEY");
        Ok(config)
    }

    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_secs.max(1))
    }

    /// Учётные данные клиента. Должны быть заданы оба поля или ни одно (тогда — анонимная схема v2).
    pub fn identity(&self) -> Result<Option<Identity>, ConfigError> {
        match (&self.nrxp_secret, &self.nrxp_public_key) {
            (None, None) => Ok(None),
            (Some(secret), Some(public)) => PeerIdentity::from_hex(secret, public)
                .map(|peer| Some(Identity::Peer(peer)))
                .map_err(|e| ConfigError::Identity(e.to_string())),
            _ => Err(ConfigError::Identity(
                "`nrxp_secret` и `nrxp_public_key` задаются только вместе".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_with_defaults() {
        let config = NodeConfig::from_toml("addr = \"1.2.3.4:443\"").unwrap();
        assert_eq!(config.addr, "1.2.3.4:443");
        assert_eq!(config.decoy_sni, netrunner_core::net::DEFAULT_DECOY_HOST);
        assert!(config.identity().unwrap().is_none());
    }

    #[test]
    fn identity_needs_both_halves() {
        let mut config = NodeConfig::anonymous("x:1");
        config.nrxp_secret = Some("00".repeat(32));
        assert!(config.identity().is_err());
        config.nrxp_public_key = Some("11".repeat(32));
        assert!(config.identity().unwrap().is_some());
        config.nrxp_public_key = Some("zz".into());
        assert!(config.identity().is_err());
    }

    #[test]
    fn debug_hides_secrets() {
        let mut config = NodeConfig::anonymous("x:1");
        config.nrxp_secret = Some("super-secret".into());
        config.auth_token = Some("token-value".into());
        let printed = format!("{config:?}");
        assert!(!printed.contains("super-secret") && !printed.contains("token-value"));
    }
}
