use nrb_transport::NodeConfig;
use url::Url;

use crate::{Error, Result};

/// Приглашение: всё, что нужно новому устройству, чтобы подключиться к узлу. **Содержит секреты** (общий секрет узла и токен
/// устройства) — передавать только по доверенному каналу; токен одноразово показывается при создании.
#[derive(Clone, PartialEq, Eq)]
pub struct Invite {
    pub host: String,
    pub port: u16,
    /// Публичный статический ключ узла (hex 64).
    pub public_key: String,
    /// `nrxp_secret` узла (hex 64).
    pub secret: String,
    /// Токен устройства (hex 64).
    pub token: String,
    pub decoy_sni: String,
    pub name: String,
}

impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invite")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Invite {
    pub const SCHEME: &'static str = "nrxp";

    pub fn to_url(&self) -> String {
        let mut url = Url::parse(&format!(
            "{}://{}:{}",
            Self::SCHEME,
            host_for_url(&self.host),
            self.port
        ))
        .expect("валидный URL приглашения");
        url.query_pairs_mut()
            .append_pair("pk", &self.public_key)
            .append_pair("secret", &self.secret)
            .append_pair("token", &self.token)
            .append_pair("sni", &self.decoy_sni)
            .append_pair("name", &self.name);
        url.into()
    }

    pub fn parse(text: &str) -> Result<Self> {
        let url = Url::parse(text.trim()).map_err(|e| Error::Invite(format!("не URL: {e}")))?;
        if url.scheme() != Self::SCHEME {
            return Err(Error::Invite(format!(
                "ожидалась схема {}://",
                Self::SCHEME
            )));
        }
        let host = url
            .host_str()
            .ok_or_else(|| Error::Invite("нет адреса узла".into()))?
            .trim_matches(['[', ']'])
            .to_owned();
        let port = url
            .port()
            .ok_or_else(|| Error::Invite("нет порта".into()))?;
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let get = |key: &str| {
            query
                .get(key)
                .cloned()
                .ok_or_else(|| Error::Invite(format!("нет поля `{key}`")))
        };
        let hex64 = |key: &str, value: String| {
            (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
                .then_some(value)
                .ok_or_else(|| {
                    Error::Invite(format!("поле `{key}` должно быть hex из 64 символов"))
                })
        };
        Ok(Self {
            host,
            port,
            public_key: hex64("pk", get("pk")?)?,
            secret: hex64("secret", get("secret")?)?,
            token: hex64("token", get("token")?)?,
            decoy_sni: get("sni")
                .unwrap_or_else(|_| netrunner_core::net::DEFAULT_DECOY_HOST.to_owned()),
            name: get("name").unwrap_or_else(|_| "device".to_owned()),
        })
    }

    /// Конфиг клиента для `NrxpTransport::connect`.
    pub fn to_node_config(&self) -> NodeConfig {
        let mut config =
            NodeConfig::anonymous(format!("{}:{}", host_for_url(&self.host), self.port));
        config.decoy_sni = self.decoy_sni.clone();
        config.auth_token = Some(self.token.clone());
        config.nrxp_secret = Some(self.secret.clone());
        config.nrxp_public_key = Some(self.public_key.clone());
        config
    }

    /// TOML для `--node файл.toml` (формат `NodeConfig`); файл содержит секреты — писать с правами 0600.
    pub fn to_config_toml(&self) -> String {
        let config = self.to_node_config();
        format!(
            "# Спарено приглашением «{}». Файл содержит секреты узла — не публиковать.\naddr = {:?}\ndecoy_sni = {:?}\nauth_token = {:?}\nnrxp_secret = {:?}\nnrxp_public_key = {:?}\n",
            self.name,
            config.addr,
            config.decoy_sni,
            config.auth_token.unwrap_or_default(),
            config.nrxp_secret.unwrap_or_default(),
            config.nrxp_public_key.unwrap_or_default(),
        )
    }
}

fn host_for_url(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Invite {
        Invite {
            host: "203.0.113.7".into(),
            port: 443,
            public_key: "ab".repeat(32),
            secret: "cd".repeat(32),
            token: "ef".repeat(32),
            decoy_sni: "www.debian.org".into(),
            name: "мой телефон".into(),
        }
    }

    #[test]
    fn roundtrip() {
        let invite = sample();
        let link = invite.to_url();
        assert!(link.starts_with("nrxp://203.0.113.7:443?"));
        assert_eq!(Invite::parse(&link).unwrap(), invite);
    }

    #[test]
    fn ipv6_host() {
        let mut invite = sample();
        invite.host = "2001:db8::1".into();
        let parsed = Invite::parse(&invite.to_url()).unwrap();
        assert_eq!(parsed.host, "2001:db8::1");
        assert_eq!(parsed.to_node_config().addr, "[2001:db8::1]:443");
    }

    #[test]
    fn rejects_bad_invites() {
        assert!(Invite::parse("https://example.org/").is_err());
        assert!(Invite::parse("nrxp://host").is_err(), "нет порта");
        assert!(Invite::parse("nrxp://host:1?pk=zz").is_err());
        let mut short = sample().to_url();
        short = short.replace(&"ef".repeat(32), "ef");
        assert!(Invite::parse(&short).is_err(), "короткий токен");
    }

    #[test]
    fn config_has_all_credentials_and_debug_hides_them() {
        let invite = sample();
        let config = invite.to_node_config();
        assert!(
            config.auth_token.is_some()
                && config.nrxp_secret.is_some()
                && config.nrxp_public_key.is_some()
        );
        assert!(config.identity().unwrap().is_some());
        assert!(!format!("{invite:?}").contains(&invite.token));
        let toml = invite.to_config_toml();
        assert_eq!(
            NodeConfig::from_toml(&toml).unwrap().addr,
            "203.0.113.7:443"
        );
    }
}
