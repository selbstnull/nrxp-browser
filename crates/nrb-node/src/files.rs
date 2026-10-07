use std::path::{Path, PathBuf};

use netrunner_core::LocalIdentity;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, random_hex, write_private};

/// Идентичность и параметры узла (`node.toml`). **Содержит секреты** — файл создаётся с правами 0600.
#[derive(Clone, Serialize, Deserialize)]
pub struct NodeFile {
    /// Адрес, который попадает в приглашения (то, что клиенты набирают снаружи).
    pub public_host: String,
    /// На каком адресе слушать.
    pub listen: String,
    pub port: u16,
    /// Домен-декой для stealth-fallback («не наши» соединения прозрачно уходят туда).
    pub decoy_host: String,
    /// `nrxp_secret` — общий секрет входа, hex 64.
    pub secret: String,
    /// Приватный ключ статической пары X25519, hex 64. Наружу не выходит.
    pub private_key: String,
    /// Строгий режим: отвергать клиентов без аутентифицированного хендшейка (всегда включён для новых узлов).
    pub strict: bool,
}

impl std::fmt::Debug for NodeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeFile")
            .field("public_host", &self.public_host)
            .field("listen", &self.listen)
            .field("port", &self.port)
            .field("decoy_host", &self.decoy_host)
            .field("secret", &"[REDACTED]")
            .field("private_key", &"[REDACTED]")
            .field("strict", &self.strict)
            .finish()
    }
}

impl NodeFile {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("node.toml")
    }

    /// Создаёт новый узел: свежие секрет и статический ключ. Существующий файл не затирается.
    pub fn init(dir: &Path, public_host: &str, port: u16) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = Self::path(dir);
        if path.exists() {
            return Err(Error::Node(format!(
                "{} уже существует — не затираю секреты узла",
                path.display()
            )));
        }
        let node = Self {
            public_host: public_host.to_owned(),
            listen: "0.0.0.0".to_owned(),
            port,
            decoy_host: netrunner_core::net::DEFAULT_DECOY_HOST.to_owned(),
            secret: random_hex(32),
            private_key: random_hex(32),
            strict: true,
        };
        node.save(dir)?;
        Ok(node)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            Error::Node(format!(
                "{}: {e} (узел создаётся командой `nrb node init`)",
                path.display()
            ))
        })?;
        let node: Self =
            toml::from_str(&text).map_err(|e| Error::Format(format!("{}: {e}", path.display())))?;
        node.identity()?; // ключи валидны
        Ok(node)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| Error::Format(e.to_string()))?;
        write_private(&Self::path(dir), &text)?;
        Ok(())
    }

    pub fn identity(&self) -> Result<LocalIdentity> {
        LocalIdentity::from_hex(&self.secret, &self.private_key, self.strict)
            .map_err(|e| Error::Format(format!("ключи узла: {e}")))
    }

    /// Публичная половина статической пары — её получают клиенты (в приглашении).
    pub fn public_key(&self) -> Result<String> {
        Ok(self.identity()?.public_key_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_private_file_and_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let node = NodeFile::init(dir.path(), "203.0.113.5", 8443).unwrap();
        assert_eq!(node.secret.len(), 64);
        assert_eq!(node.private_key.len(), 64);
        assert!(node.strict);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(NodeFile::path(dir.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "файл с секретами должен быть закрыт от других пользователей"
            );
        }
        assert!(
            NodeFile::init(dir.path(), "x", 1).is_err(),
            "повторный init не должен затирать секреты"
        );
        let loaded = NodeFile::load(dir.path()).unwrap();
        assert_eq!(loaded.secret, node.secret);
        assert_eq!(loaded.public_key().unwrap().len(), 64);
    }

    #[test]
    fn debug_hides_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let node = NodeFile::init(dir.path(), "h", 1).unwrap();
        let printed = format!("{node:?}");
        assert!(!printed.contains(&node.secret) && !printed.contains(&node.private_key));
    }

    #[test]
    fn broken_keys_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        NodeFile::init(dir.path(), "h", 1).unwrap();
        let path = NodeFile::path(dir.path());
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("private_key = \"", "private_key = \"zz");
        std::fs::write(&path, text).unwrap();
        assert!(NodeFile::load(dir.path()).is_err());
    }
}
