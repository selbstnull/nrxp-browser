//! Хранилище паролей (менеджер паролей, v1). Один файл `vault.nrbv` в профиле; всё содержимое (записи, список «никогда не
//! предлагать») — один зашифрованный блок.
//!
//! Модель угроз: файл попадает к постороннему (украденный диск, облачная копия профиля) — без мастер-пароля содержимое
//! недоступно, перебор замедлен Argon2id (по умолчанию 64 МиБ × 3 прохода). НЕ защищает от вредоносной программы под тем же
//! пользователем, пока хранилище разблокировано: расшифрованные записи лежат в памяти процесса (после `lock()` и при
//! `Drop` затираются `zeroize`).
//!
//! Формат файла: `NRBV1` | m_kib u32 | t u32 | p u32 (все big-endian) | соль 16 | nonce 24 | шифртекст+тег. Параметры KDF
//! и заголовок идут как AAD, поэтому подмена параметров (понижение стоимости) ломает расшифровку.

use std::io::Write;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 5] = b"NRBV1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 5 + 12 + SALT_LEN + NONCE_LEN;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("хранилище заблокировано")]
    Locked,
    #[error("неверный мастер-пароль или повреждённый файл")]
    BadPasswordOrCorrupt,
    #[error("хранилища ещё нет")]
    NoVault,
    #[error("хранилище уже создано")]
    AlreadyExists,
    #[error("мастер-пароль слишком короткий (минимум 8 символов)")]
    WeakPassword,
    #[error("формат файла не поддерживается")]
    Format,
    #[error("ввод-вывод: {0}")]
    Io(#[from] std::io::Error),
    #[error("криптография: {0}")]
    Crypto(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Стоимость KDF. Хранится в файле (старые файлы читаются со своими параметрами).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        // 64 МиБ, 3 прохода, 1 поток: порядка 0,3–0,6 с на обычном ноутбуке; разблокировка раз за сеанс.
        Self {
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 1,
        }
    }
}

impl KdfParams {
    /// Дёшево — только для тестов.
    pub fn insecure_fast() -> Self {
        Self {
            memory_kib: 8,
            iterations: 1,
            parallelism: 1,
        }
    }

    fn sane(self) -> bool {
        // Защита от файла с чудовищными параметрами (отказ в обслуживании при открытии): верхние границы.
        (8..=1024 * 1024).contains(&self.memory_kib)
            && (1..=16).contains(&self.iterations)
            && (1..=8).contains(&self.parallelism)
    }
}

/// Сохранённый вход.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Entry {
    pub id: u64,
    /// Источник: `схема://хост[:порт]` — совпадение только точное (без поддоменов).
    pub origin: String,
    pub username: String,
    pub password: String,
    /// Unix-время создания/последнего изменения, секунды.
    pub created: u64,
    pub updated: u64,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Content {
    next_id: u64,
    entries: Vec<Entry>,
    /// Источники, для которых браузер не предлагает сохранение.
    never: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    Added,
    Updated,
    /// Такая запись уже есть.
    Unchanged,
}

struct Unlocked {
    key: Zeroizing<[u8; 32]>,
    salt: [u8; SALT_LEN],
    params: KdfParams,
    content: Content,
}

pub struct Vault {
    path: PathBuf,
    state: Option<Unlocked>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| Error::Crypto(format!("случайные байты: {e}")))?;
    Ok(bytes)
}

fn derive(password: &str, salt: &[u8], params: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let argon_params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(32),
    )
    .map_err(|e| Error::Crypto(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key[..])
        .map_err(|e| Error::Crypto(e.to_string()))?;
    Ok(key)
}

fn header(params: KdfParams, salt: &[u8; SALT_LEN], nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&params.memory_kib.to_be_bytes());
    out.extend_from_slice(&params.iterations.to_be_bytes());
    out.extend_from_slice(&params.parallelism.to_be_bytes());
    out.extend_from_slice(salt);
    out.extend_from_slice(nonce);
    out
}

impl Vault {
    /// Открывает (не расшифровывая) хранилище в каталоге профиля. Файла может не быть.
    pub fn open(dir: &Path) -> Self {
        Self {
            path: dir.join("vault.nrbv"),
            state: None,
        }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn is_unlocked(&self) -> bool {
        self.state.is_some()
    }

    /// Создаёт пустое хранилище с мастер-паролем и оставляет его разблокированным.
    pub fn create(&mut self, master: &str, params: KdfParams) -> Result<()> {
        if self.exists() {
            return Err(Error::AlreadyExists);
        }
        if master.chars().count() < 8 {
            return Err(Error::WeakPassword);
        }
        let salt = random::<SALT_LEN>()?;
        let key = derive(master, &salt, params)?;
        self.state = Some(Unlocked {
            key,
            salt,
            params,
            content: Content::default(),
        });
        self.save()
    }

    pub fn unlock(&mut self, master: &str) -> Result<()> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::NoVault),
            Err(e) => return Err(e.into()),
        };
        if bytes.len() < HEADER_LEN + 16 || &bytes[..5] != MAGIC {
            return Err(Error::Format);
        }
        let u32_at = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 байта"));
        let params = KdfParams {
            memory_kib: u32_at(5),
            iterations: u32_at(9),
            parallelism: u32_at(13),
        };
        if !params.sane() {
            return Err(Error::Format);
        }
        let salt: [u8; SALT_LEN] = bytes[17..17 + SALT_LEN].try_into().expect("соль");
        let nonce: [u8; NONCE_LEN] = bytes[17 + SALT_LEN..HEADER_LEN].try_into().expect("nonce");
        let key = derive(master, &salt, params)?;
        let cipher = XChaCha20Poly1305::new(key.as_slice().into());
        let plain = cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &bytes[HEADER_LEN..],
                    aad: &bytes[..HEADER_LEN],
                },
            )
            .map_err(|_| Error::BadPasswordOrCorrupt)?;
        let plain = Zeroizing::new(plain);
        let content: Content = serde_json::from_slice(&plain).map_err(|_| Error::Format)?;
        self.state = Some(Unlocked {
            key,
            salt,
            params,
            content,
        });
        Ok(())
    }

    /// Забывает ключ и расшифрованные записи (они затираются при `Drop`).
    pub fn lock(&mut self) {
        self.state = None;
    }

    fn open_state(&self) -> Result<&Unlocked> {
        self.state.as_ref().ok_or(Error::Locked)
    }

    fn open_state_mut(&mut self) -> Result<&mut Unlocked> {
        self.state.as_mut().ok_or(Error::Locked)
    }

    /// Записывает файл атомарно (временный файл с правами 0600 + rename); для каждой записи — новый nonce.
    fn save(&self) -> Result<()> {
        let state = self.open_state()?;
        let plain = Zeroizing::new(
            serde_json::to_vec(&state.content).map_err(|e| Error::Crypto(e.to_string()))?,
        );
        let nonce = random::<NONCE_LEN>()?;
        let head = header(state.params, &state.salt, &nonce);
        let cipher = XChaCha20Poly1305::new(state.key.as_slice().into());
        let sealed = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: &head,
                },
            )
            .map_err(|e| Error::Crypto(e.to_string()))?;
        let mut file = head;
        file.extend_from_slice(&sealed);

        let tmp = self.path.with_extension("nrbv.tmp");
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut out = options.open(&tmp)?;
            out.write_all(&file)?;
            out.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Записи для точного источника.
    pub fn logins_for(&self, origin: &str) -> Result<Vec<Entry>> {
        Ok(self
            .open_state()?
            .content
            .entries
            .iter()
            .filter(|e| e.origin == origin)
            .cloned()
            .collect())
    }

    pub fn all(&self) -> Result<Vec<Entry>> {
        Ok(self.open_state()?.content.entries.clone())
    }

    /// Что произойдёт при сохранении входа (для текста вопроса «Сохранить/Обновить пароль?»), без записи.
    pub fn preview_save(
        &self,
        origin: &str,
        username: &str,
        password: &str,
    ) -> Result<SaveOutcome> {
        let state = self.open_state()?;
        Ok(
            match state
                .content
                .entries
                .iter()
                .find(|e| e.origin == origin && e.username == username)
            {
                None => SaveOutcome::Added,
                Some(e) if e.password == password => SaveOutcome::Unchanged,
                Some(_) => SaveOutcome::Updated,
            },
        )
    }

    pub fn save_login(
        &mut self,
        origin: &str,
        username: &str,
        password: &str,
    ) -> Result<SaveOutcome> {
        let outcome = self.preview_save(origin, username, password)?;
        if outcome == SaveOutcome::Unchanged {
            return Ok(outcome);
        }
        let state = self.open_state_mut()?;
        let stamp = now();
        match state
            .content
            .entries
            .iter_mut()
            .find(|e| e.origin == origin && e.username == username)
        {
            Some(entry) => {
                entry.password.zeroize();
                entry.password = password.to_owned();
                entry.updated = stamp;
            }
            None => {
                let id = state.content.next_id;
                state.content.next_id += 1;
                state.content.entries.push(Entry {
                    id,
                    origin: origin.to_owned(),
                    username: username.to_owned(),
                    password: password.to_owned(),
                    created: stamp,
                    updated: stamp,
                });
            }
        }
        self.save()?;
        Ok(outcome)
    }

    pub fn delete(&mut self, id: u64) -> Result<bool> {
        let state = self.open_state_mut()?;
        let before = state.content.entries.len();
        state.content.entries.retain(|e| e.id != id);
        let removed = state.content.entries.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn never_save_for(&self, origin: &str) -> Result<bool> {
        Ok(self.open_state()?.content.never.iter().any(|o| o == origin))
    }

    pub fn set_never_save(&mut self, origin: &str, never: bool) -> Result<()> {
        let state = self.open_state_mut()?;
        state.content.never.retain(|o| o != origin);
        if never {
            state.content.never.push(origin.to_owned());
        }
        self.save()
    }

    /// Меняет мастер-пароль: новая соль и ключ, файл перезаписывается.
    pub fn change_master(&mut self, old: &str, new: &str) -> Result<()> {
        if new.chars().count() < 8 {
            return Err(Error::WeakPassword);
        }
        // Проверяем старый пароль заново расшифровкой файла, а не сравнением с чем-то в памяти.
        let mut probe = Vault::open(self.path.parent().unwrap_or_else(|| Path::new(".")));
        probe.unlock(old)?;
        let params = self.open_state()?.params;
        let salt = random::<SALT_LEN>()?;
        let key = derive(new, &salt, params)?;
        let state = self.open_state_mut()?;
        state.salt = salt;
        state.key = key;
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = Vault::open(dir.path());
        vault
            .create("correct horse", KdfParams::insecure_fast())
            .unwrap();
        (dir, vault)
    }

    #[test]
    fn roundtrip_and_persistence() {
        let (dir, mut vault) = fresh();
        assert_eq!(
            vault.save_login("https://a.test", "ann", "p1").unwrap(),
            SaveOutcome::Added
        );
        assert_eq!(
            vault.save_login("https://a.test", "ann", "p1").unwrap(),
            SaveOutcome::Unchanged
        );
        assert_eq!(
            vault.save_login("https://a.test", "ann", "p2").unwrap(),
            SaveOutcome::Updated
        );
        vault
            .save_login("https://a.test:8443", "ann", "other")
            .unwrap();
        vault.lock();
        assert!(matches!(
            vault.logins_for("https://a.test"),
            Err(Error::Locked)
        ));

        let mut again = Vault::open(dir.path());
        assert!(matches!(
            again.unlock("wrong wrong"),
            Err(Error::BadPasswordOrCorrupt)
        ));
        again.unlock("correct horse").unwrap();
        let logins = again.logins_for("https://a.test").unwrap();
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].password, "p2");
        // Порт — часть источника.
        assert_eq!(again.logins_for("https://a.test:8443").unwrap().len(), 1);
        assert!(again.logins_for("https://sub.a.test").unwrap().is_empty());
    }

    #[test]
    fn file_does_not_contain_plaintext_and_tampering_is_detected() {
        let (dir, mut vault) = fresh();
        vault
            .save_login(
                "https://secret-origin.test",
                "very-unique-user",
                "very-unique-pass",
            )
            .unwrap();
        let path = dir.path().join("vault.nrbv");
        let bytes = std::fs::read(&path).unwrap();
        for needle in ["secret-origin", "very-unique-user", "very-unique-pass"] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{needle} лежит открытым текстом"
            );
        }
        // Подмена параметров KDF в заголовке (понижение стоимости) ломает расшифровку.
        let mut weakened = bytes.clone();
        weakened[8] ^= 1;
        std::fs::write(&path, &weakened).unwrap();
        let mut v = Vault::open(dir.path());
        assert!(v.unlock("correct horse").is_err());
        // Порча шифртекста.
        let mut broken = bytes;
        let last = broken.len() - 1;
        broken[last] ^= 1;
        std::fs::write(&path, &broken).unwrap();
        assert!(matches!(
            Vault::open(dir.path()).unlock("correct horse"),
            Err(Error::BadPasswordOrCorrupt)
        ));
    }

    #[test]
    fn nonce_is_fresh_for_every_save() {
        let (dir, mut vault) = fresh();
        vault.save_login("https://a.test", "u", "1").unwrap();
        let first = std::fs::read(dir.path().join("vault.nrbv")).unwrap();
        vault.save_login("https://a.test", "u", "2").unwrap();
        let second = std::fs::read(dir.path().join("vault.nrbv")).unwrap();
        assert_ne!(
            &first[17 + SALT_LEN..HEADER_LEN],
            &second[17 + SALT_LEN..HEADER_LEN]
        );
    }

    #[test]
    fn master_password_rules_and_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = Vault::open(dir.path());
        assert!(matches!(
            vault.create("short", KdfParams::insecure_fast()),
            Err(Error::WeakPassword)
        ));
        vault
            .create("long enough pw", KdfParams::insecure_fast())
            .unwrap();
        assert!(matches!(
            vault.create("another long pw", KdfParams::insecure_fast()),
            Err(Error::AlreadyExists)
        ));
        vault.save_login("https://a.test", "u", "p").unwrap();
        assert!(matches!(
            vault.change_master("not the pw", "new long password"),
            Err(Error::BadPasswordOrCorrupt)
        ));
        vault
            .change_master("long enough pw", "new long password")
            .unwrap();
        let mut again = Vault::open(dir.path());
        assert!(again.unlock("long enough pw").is_err());
        again.unlock("new long password").unwrap();
        assert_eq!(again.all().unwrap().len(), 1);
    }

    #[test]
    fn delete_never_save_and_absurd_kdf_params() {
        let (dir, mut vault) = fresh();
        vault.save_login("https://a.test", "u", "p").unwrap();
        let id = vault.all().unwrap()[0].id;
        assert!(vault.delete(id).unwrap());
        assert!(!vault.delete(id).unwrap());
        vault.set_never_save("https://nope.test", true).unwrap();
        assert!(vault.never_save_for("https://nope.test").unwrap());
        vault.set_never_save("https://nope.test", false).unwrap();
        assert!(!vault.never_save_for("https://nope.test").unwrap());
        // Файл с запредельной стоимостью KDF не открывается (иначе открытие подвесило бы браузер).
        let path = dir.path().join("vault.nrbv");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[5..9].copy_from_slice(&u32::MAX.to_be_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            Vault::open(dir.path()).unlock("correct horse"),
            Err(Error::Format)
        ));
    }
}
