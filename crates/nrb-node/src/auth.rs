//! Реестр спаренных устройств и `AuthValidator` без бэкенда.
//!
//! Токен устройства — 32 случайных байта; в реестре лежит только SHA-256 токена (утечка `devices.json` токенов не раскрывает).
//! Реестр перечитывается при изменении файла, поэтому `nrb node invite` и `revoke` действуют на работающий узел без перезапуска.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use netrunner_core::net::{AuthValidator, NodeHealthReport, UsageDelta, UsageReport, UserQuota};
use netrunner_logger::{AppError, ERR_AUTH_FAILED};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result, now_unix, random_hex, write_private};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Короткий идентификатор (hex 8) — он же `user_id` для ядра и ключ учёта трафика.
    pub id: String,
    pub name: String,
    pub token_sha256: String,
    pub created: i64,
    #[serde(default)]
    pub revoked: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    devices: Vec<Device>,
}

struct Cache {
    registry: Registry,
    mtime: Option<SystemTime>,
    checked: Instant,
}

pub struct DeviceStore {
    path: PathBuf,
    cache: Mutex<Cache>,
}

fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

impl DeviceStore {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("devices.json")
    }

    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = Self::path(dir);
        let store = Self {
            path,
            cache: Mutex::new(Cache {
                registry: Registry::default(),
                mtime: None,
                checked: Instant::now(),
            }),
        };
        store.reload(true)?;
        Ok(store)
    }

    fn mtime(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
    }

    /// Перечитывает файл, если он изменился (проверка не чаще раза в секунду); `force` — безусловно.
    fn reload(&self, force: bool) -> Result<()> {
        let mut cache = self.cache.lock().expect("mutex реестра");
        if !force && cache.checked.elapsed() < std::time::Duration::from_secs(1) {
            return Ok(());
        }
        cache.checked = Instant::now();
        let mtime = self.mtime();
        if !force && mtime == cache.mtime {
            return Ok(());
        }
        cache.registry = match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| Error::Format(format!("{}: {e}", self.path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Registry::default(),
            Err(e) => return Err(e.into()),
        };
        cache.mtime = mtime;
        Ok(())
    }

    fn persist(&self, registry: &Registry) -> Result<()> {
        let text =
            serde_json::to_string_pretty(registry).map_err(|e| Error::Format(e.to_string()))?;
        write_private(&self.path, &text)?;
        let mut cache = self.cache.lock().expect("mutex реестра");
        cache.mtime = self.mtime();
        Ok(())
    }

    /// Добавляет устройство. Возвращает устройство и **токен — показывается один раз**, в реестре остаётся только хеш.
    pub fn add_device(&self, name: &str) -> Result<(Device, String)> {
        self.reload(true)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Node("имя устройства не может быть пустым".into()));
        }
        let mut registry = {
            let cache = self.cache.lock().expect("mutex реестра");
            Registry {
                devices: cache.registry.devices.clone(),
            }
        };
        if registry
            .devices
            .iter()
            .any(|d| d.name == name && !d.revoked)
        {
            return Err(Error::DeviceExists(name.to_owned()));
        }
        let token = random_hex(32);
        let device = Device {
            id: random_hex(4),
            name: name.to_owned(),
            token_sha256: hash_token(&token),
            created: now_unix(),
            revoked: false,
        };
        registry.devices.push(device.clone());
        self.persist(&registry)?;
        self.cache.lock().expect("mutex реестра").registry = registry;
        Ok((device, token))
    }

    /// Отзывает устройство по имени. Возвращает его `id` (чтобы закрыть живые сессии).
    pub fn revoke(&self, name: &str) -> Result<String> {
        self.reload(true)?;
        let mut registry = {
            let cache = self.cache.lock().expect("mutex реестра");
            Registry {
                devices: cache.registry.devices.clone(),
            }
        };
        let device = registry
            .devices
            .iter_mut()
            .find(|d| d.name == name && !d.revoked)
            .ok_or_else(|| Error::NoSuchDevice(name.to_owned()))?;
        device.revoked = true;
        let id = device.id.clone();
        self.persist(&registry)?;
        self.cache.lock().expect("mutex реестра").registry = registry;
        Ok(id)
    }

    pub fn list(&self) -> Result<Vec<Device>> {
        self.reload(true)?;
        Ok(self
            .cache
            .lock()
            .expect("mutex реестра")
            .registry
            .devices
            .clone())
    }

    /// Устройство по токену (только не отозванное).
    pub fn find_by_token(&self, token: &str) -> Option<Device> {
        let _ = self.reload(false);
        let hash = hash_token(token);
        let cache = self.cache.lock().expect("mutex реестра");
        cache
            .registry
            .devices
            .iter()
            .find(|d| !d.revoked && constant_time_eq(&d.token_sha256, &hash))
            .cloned()
    }

    /// Идентификаторы отозванных устройств (для закрытия живых сессий).
    pub fn revoked_ids(&self) -> Vec<String> {
        let _ = self.reload(false);
        let cache = self.cache.lock().expect("mutex реестра");
        cache
            .registry
            .devices
            .iter()
            .filter(|d| d.revoked)
            .map(|d| d.id.clone())
            .collect()
    }
}

/// `AuthValidator` поверх локального реестра: никакого бэкенда, токены — устройства, спаренные приглашением.
pub struct LocalAuth {
    store: std::sync::Arc<DeviceStore>,
    /// Трафик по устройствам с момента старта (байты); для статистики узла.
    usage: Mutex<HashMap<String, u64>>,
}

impl LocalAuth {
    pub fn new(store: std::sync::Arc<DeviceStore>) -> Self {
        Self {
            store,
            usage: Mutex::new(HashMap::new()),
        }
    }

    pub fn usage(&self) -> HashMap<String, u64> {
        self.usage.lock().expect("mutex учёта").clone()
    }
}

#[async_trait]
impl AuthValidator for LocalAuth {
    async fn validate(&self, token: &str) -> std::result::Result<UserQuota, AppError> {
        match self.store.find_by_token(token) {
            Some(device) => Ok(UserQuota {
                user_id: device.id,
                limit_bytes: None,
                used_bytes: 0,
            }),
            None => Err(AppError::new(
                ERR_AUTH_FAILED,
                "Устройство не спарено",
                "Unknown or revoked device token",
            )),
        }
    }

    async fn report_usage(
        &self,
        user_id: &str,
        delta_bytes: u64,
    ) -> std::result::Result<UsageReport, AppError> {
        let mut usage = self.usage.lock().expect("mutex учёта");
        let used = usage.entry(user_id.to_owned()).or_default();
        *used = used.saturating_add(delta_bytes);
        Ok(UsageReport {
            used_bytes: *used,
            limit_bytes: None,
            over_limit: false,
        })
    }

    async fn report_usage_batch(
        &self,
        _batch_id: &str,
        deltas: &[UsageDelta],
    ) -> std::result::Result<HashMap<String, UsageReport>, AppError> {
        let mut reports = HashMap::new();
        for (user_id, delta) in deltas {
            reports.insert(user_id.clone(), self.report_usage(user_id, *delta).await?);
        }
        Ok(reports)
    }

    async fn report_node_health(
        &self,
        _report: NodeHealthReport,
    ) -> std::result::Result<(), AppError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_find_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeviceStore::open(dir.path()).unwrap();
        let (device, token) = store.add_device("телефон").unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(store.find_by_token(&token).unwrap().id, device.id);
        assert!(store.find_by_token("не тот токен").is_none());
        assert!(matches!(
            store.add_device("телефон"),
            Err(Error::DeviceExists(_))
        ));

        let id = store.revoke("телефон").unwrap();
        assert_eq!(id, device.id);
        assert!(
            store.find_by_token(&token).is_none(),
            "отозванный токен перестаёт работать"
        );
        assert_eq!(store.revoked_ids(), vec![id]);
        assert!(
            store.add_device("телефон").is_ok(),
            "после отзыва имя можно занять заново"
        );
        assert!(matches!(
            store.revoke("нет такого"),
            Err(Error::NoSuchDevice(_))
        ));
    }

    #[test]
    fn registry_keeps_only_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeviceStore::open(dir.path()).unwrap();
        let (_, token) = store.add_device("ноутбук").unwrap();
        let on_disk = std::fs::read_to_string(DeviceStore::path(dir.path())).unwrap();
        assert!(!on_disk.contains(&token), "сам токен на диск не пишется");
        assert!(on_disk.contains(&hash_token(&token)));
    }

    #[test]
    fn another_process_changes_are_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        let running = DeviceStore::open(dir.path()).unwrap(); // как «работающий узел»
        let cli = DeviceStore::open(dir.path()).unwrap(); // как `nrb node invite` в другом процессе
        let (_, token) = cli.add_device("планшет").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100)); // интервал перепроверки файла — 1 с
        assert!(
            running.find_by_token(&token).is_some(),
            "узел видит новое устройство без перезапуска"
        );
        cli.revoke("планшет").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(running.find_by_token(&token).is_none(), "и отзыв тоже");
    }

    #[tokio::test]
    async fn local_auth_validates_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(DeviceStore::open(dir.path()).unwrap());
        let (device, token) = store.add_device("d").unwrap();
        let auth = LocalAuth::new(store);
        assert_eq!(auth.validate(&token).await.unwrap().user_id, device.id);
        assert!(auth.validate("bad").await.is_err());
        auth.report_usage(&device.id, 100).await.unwrap();
        let report = auth.report_usage(&device.id, 50).await.unwrap();
        assert_eq!(report.used_bytes, 150);
        assert!(!report.over_limit);
    }
}
