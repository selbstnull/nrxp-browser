use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{DeployError, Result};

/// Пины отпечатков SSH-хост-ключей (`known_hosts.toml` в каталоге данных nrb): `host:port` → `SHA256:…`.
/// Свой файл, а не `~/.ssh/known_hosts`: формат OpenSSH с хешированными именами нам не нужен, а чужой файл не трогаем.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct KnownHosts {
    #[serde(default)]
    hosts: BTreeMap<String, String>,
    #[serde(skip)]
    path: PathBuf,
}

impl KnownHosts {
    pub fn load(path: &Path) -> Result<Self> {
        let mut known: Self = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| DeployError::Local(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(DeployError::Local(format!("{}: {e}", path.display()))),
        };
        known.path = path.to_path_buf();
        Ok(known)
    }

    pub fn get(&self, host: &str, port: u16) -> Option<&str> {
        self.hosts
            .get(&format!("{host}:{port}"))
            .map(String::as_str)
    }

    pub fn pin(&mut self, host: &str, port: u16, fingerprint: &str) -> Result<()> {
        self.hosts
            .insert(format!("{host}:{port}"), fingerprint.to_owned());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DeployError::Local(e.to_string()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| DeployError::Local(e.to_string()))?;
        std::fs::write(&self.path, text)
            .map_err(|e| DeployError::Local(format!("{}: {e}", self.path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_persists_and_is_per_host_and_port() {
        let dir = std::env::temp_dir().join(format!("nrb-kh-{}", std::process::id()));
        let path = dir.join("known_hosts.toml");
        let mut known = KnownHosts::load(&path).unwrap();
        assert!(known.get("203.0.113.9", 22).is_none());
        known.pin("203.0.113.9", 22, "SHA256:abc").unwrap();
        let reloaded = KnownHosts::load(&path).unwrap();
        assert_eq!(reloaded.get("203.0.113.9", 22), Some("SHA256:abc"));
        assert!(
            reloaded.get("203.0.113.9", 2222).is_none(),
            "порт — часть ключа"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
