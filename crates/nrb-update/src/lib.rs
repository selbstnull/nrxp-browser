//! Подписанный канал обновлений (I14). Релиз описывается **манифестом** (JSON: версия, артефакты с SHA-256 и размером, адреса),
//! манифест подписывается ed25519 **как есть, побайтно** (отдельный файл подписи); клиент и сервер деплоя проверяют подпись
//! публичным ключом, версию (откат запрещён без явного разрешения) и хеш артефакта.
//!
//! Доверие строится только на публичном ключе: адрес, с которого пришёл манифест, не доверенный. Секретный ключ хранится у
//! выпускающего (`nrb release keygen`), в сборку не попадает.

use std::fmt::Write as _;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("подпись манифеста неверна: манифест изменён или подписан другим ключом")]
    BadSignature,
    #[error("ключ или подпись: {0}")]
    Encoding(String),
    #[error("манифест: {0}")]
    Manifest(String),
    #[error("версия {offered} не новее установленной {current} (откат запрещён)")]
    Downgrade { current: String, offered: String },
    #[error("хеш артефакта не совпадает с манифестом (ожидался {expected}, получен {actual})")]
    HashMismatch { expected: String, actual: String },
    #[error("артефакта «{0}» нет в манифесте")]
    NoArtifact(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    /// SHA-256 содержимого, hex в нижнем регистре.
    pub sha256: String,
    pub size: u64,
    /// Откуда скачать (сам адрес не доверенный: после загрузки проверяется хеш).
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub channel: String,
    /// Метка времени публикации (unix, секунды).
    #[serde(default)]
    pub published: i64,
    #[serde(default)]
    pub notes: String,
    pub artifacts: Vec<Artifact>,
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(data) {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hex_decode(text: &str) -> Result<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return Err(Error::Encoding("нечётная длина hex".into()));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| Error::Encoding("не hex".into()))
        })
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Пара ключей: секретный (hex 64) и публичный (hex 64).
pub fn generate_keypair() -> (String, String) {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("системный генератор случайных чисел");
    let key = SigningKey::from_bytes(&seed);
    (
        hex_encode(&seed),
        hex_encode(key.verifying_key().as_bytes()),
    )
}

fn signing_key(secret_hex: &str) -> Result<SigningKey> {
    let bytes: [u8; 32] = hex_decode(secret_hex)?
        .try_into()
        .map_err(|_| Error::Encoding("секретный ключ — 32 байта (hex 64)".into()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn verifying_key(public_hex: &str) -> Result<VerifyingKey> {
    let bytes: [u8; 32] = hex_decode(public_hex)?
        .try_into()
        .map_err(|_| Error::Encoding("публичный ключ — 32 байта (hex 64)".into()))?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| Error::Encoding(e.to_string()))
}

/// Подпись ровно этих байт манифеста; результат — hex.
pub fn sign(manifest_bytes: &[u8], secret_hex: &str) -> Result<String> {
    Ok(hex_encode(
        &signing_key(secret_hex)?.sign(manifest_bytes).to_bytes(),
    ))
}

/// Проверяет подпись **до** разбора: недоверенный JSON парсится только после успешной проверки.
pub fn verify_manifest(
    manifest_bytes: &[u8],
    signature_hex: &str,
    public_hex: &str,
) -> Result<Manifest> {
    let signature: [u8; 64] = hex_decode(signature_hex)?
        .try_into()
        .map_err(|_| Error::Encoding("подпись — 64 байта (hex 128)".into()))?;
    verifying_key(public_hex)?
        .verify(manifest_bytes, &Signature::from_bytes(&signature))
        .map_err(|_| Error::BadSignature)?;
    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).map_err(|e| Error::Manifest(e.to_string()))?;
    if parse_version(&manifest.version).is_none() {
        return Err(Error::Manifest(format!(
            "версия «{}» не в формате MAJOR.MINOR.PATCH[-pre]",
            manifest.version
        )));
    }
    for artifact in &manifest.artifacts {
        if artifact.name.is_empty()
            || artifact.name.contains(['/', '\\'])
            || artifact.sha256.len() != 64
            || hex_decode(&artifact.sha256).is_err()
        {
            return Err(Error::Manifest(format!(
                "артефакт «{}»: пустое или небезопасное имя либо некорректный sha256",
                artifact.name
            )));
        }
    }
    Ok(manifest)
}

/// `MAJOR.MINOR.PATCH` с необязательным `-пререлиз`; без пререлиза версия новее такой же с ним.
fn parse_version(text: &str) -> Option<([u64; 3], Option<String>)> {
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_owned())),
        Some(_) => return None,
        None => (text, None),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some((version, pre))
}

/// Новее ли `offered`, чем `current` (строго).
pub fn is_newer(offered: &str, current: &str) -> Result<bool> {
    let o = parse_version(offered).ok_or_else(|| Error::Manifest(format!("версия «{offered}»")))?;
    let c = parse_version(current).ok_or_else(|| Error::Manifest(format!("версия «{current}»")))?;
    Ok(match o.0.cmp(&c.0) {
        std::cmp::Ordering::Equal => match (&o.1, &c.1) {
            (None, Some(_)) => true,
            (Some(a), Some(b)) => a > b,
            _ => false,
        },
        ordering => ordering == std::cmp::Ordering::Greater,
    })
}

/// Защита от отката: предложенная версия обязана быть новее установленной (если не разрешено иное).
pub fn check_not_downgrade(
    manifest: &Manifest,
    current: &str,
    allow_downgrade: bool,
) -> Result<()> {
    if allow_downgrade || is_newer(&manifest.version, current)? {
        Ok(())
    } else {
        Err(Error::Downgrade {
            current: current.to_owned(),
            offered: manifest.version.clone(),
        })
    }
}

impl Manifest {
    pub fn artifact(&self, name: &str) -> Result<&Artifact> {
        self.artifacts
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| Error::NoArtifact(name.to_owned()))
    }

    /// Артефакт по содержимому (для деплоя: есть ли такой бинарь в проверенном манифесте).
    pub fn artifact_by_hash(&self, sha256: &str) -> Option<&Artifact> {
        self.artifacts
            .iter()
            .find(|a| a.sha256.eq_ignore_ascii_case(sha256))
    }
}

/// Проверяет содержимое скачанного файла по манифесту.
pub fn verify_artifact(manifest: &Manifest, name: &str, data: &[u8]) -> Result<()> {
    let artifact = manifest.artifact(name)?;
    let actual = sha256_hex(data);
    if actual.eq_ignore_ascii_case(&artifact.sha256) && data.len() as u64 == artifact.size {
        Ok(())
    } else {
        Err(Error::HashMismatch {
            expected: artifact.sha256.clone(),
            actual,
        })
    }
}

/// Собирает манифест (для `nrb release manifest`).
pub fn build_manifest(
    version: &str,
    channel: &str,
    notes: &str,
    published: i64,
    artifacts: Vec<Artifact>,
) -> Result<Vec<u8>> {
    if parse_version(version).is_none() {
        return Err(Error::Manifest(format!("версия «{version}»")));
    }
    let manifest = Manifest {
        version: version.to_owned(),
        channel: channel.to_owned(),
        published,
        notes: notes.to_owned(),
        artifacts,
    };
    serde_json::to_vec_pretty(&manifest).map_err(|e| Error::Manifest(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_bytes(version: &str) -> Vec<u8> {
        build_manifest(
            version,
            "stable",
            "тест",
            1,
            vec![Artifact {
                name: "nrb-linux-x86_64.tar.gz".into(),
                sha256: sha256_hex(b"payload"),
                size: 7,
                url: "https://example.org/nrb.tar.gz".into(),
            }],
        )
        .unwrap()
    }

    #[test]
    fn signed_manifest_verifies_and_any_change_breaks_it() {
        let (secret, public) = generate_keypair();
        let bytes = manifest_bytes("1.2.3");
        let signature = sign(&bytes, &secret).unwrap();
        let manifest = verify_manifest(&bytes, &signature, &public).unwrap();
        assert_eq!(manifest.version, "1.2.3");

        let mut tampered = bytes.clone();
        let at = tampered.iter().position(|b| *b == b'3').unwrap();
        tampered[at] = b'4';
        assert_eq!(
            verify_manifest(&tampered, &signature, &public).unwrap_err(),
            Error::BadSignature
        );
        // чужой ключ
        let (_, other_public) = generate_keypair();
        assert_eq!(
            verify_manifest(&bytes, &signature, &other_public).unwrap_err(),
            Error::BadSignature
        );
        // мусор вместо подписи/ключа — ошибка кодировки, а не паника
        assert!(matches!(
            verify_manifest(&bytes, "zz", &public),
            Err(Error::Encoding(_))
        ));
        assert!(matches!(
            verify_manifest(&bytes, &signature, "00"),
            Err(Error::Encoding(_))
        ));
    }

    #[test]
    fn untrusted_json_is_not_parsed_before_the_signature_is_checked() {
        let (_, public) = generate_keypair();
        let (other_secret, _) = generate_keypair();
        let garbage = "{ это не json".as_bytes();
        let signature = sign(garbage, &other_secret).unwrap();
        assert_eq!(
            verify_manifest(garbage, &signature, &public).unwrap_err(),
            Error::BadSignature,
            "сначала подпись — потом разбор"
        );
    }

    #[test]
    fn versions_compare_and_downgrades_are_refused() {
        assert!(is_newer("0.2.0", "0.1.9").unwrap());
        assert!(is_newer("1.0.0", "0.99.99").unwrap());
        assert!(!is_newer("0.1.0", "0.1.0").unwrap());
        assert!(!is_newer("0.1.0", "0.2.0").unwrap());
        assert!(
            is_newer("1.0.0", "1.0.0-rc1").unwrap(),
            "релиз новее пререлиза"
        );
        assert!(!is_newer("1.0.0-rc1", "1.0.0").unwrap());
        assert!(is_newer("1.0.0-rc2", "1.0.0-rc1").unwrap());
        assert!(is_newer("v1", "0.1.0").is_err());

        let (secret, public) = generate_keypair();
        let bytes = manifest_bytes("0.1.0");
        let manifest = verify_manifest(&bytes, &sign(&bytes, &secret).unwrap(), &public).unwrap();
        assert!(matches!(
            check_not_downgrade(&manifest, "0.2.0", false),
            Err(Error::Downgrade { .. })
        ));
        assert!(check_not_downgrade(&manifest, "0.2.0", true).is_ok());
        assert!(check_not_downgrade(&manifest, "0.0.9", false).is_ok());
    }

    #[test]
    fn artifact_hash_is_checked() {
        let (secret, public) = generate_keypair();
        let bytes = manifest_bytes("1.0.0");
        let manifest = verify_manifest(&bytes, &sign(&bytes, &secret).unwrap(), &public).unwrap();
        assert!(verify_artifact(&manifest, "nrb-linux-x86_64.tar.gz", b"payload").is_ok());
        assert!(matches!(
            verify_artifact(&manifest, "nrb-linux-x86_64.tar.gz", b"payloaD"),
            Err(Error::HashMismatch { .. })
        ));
        assert!(matches!(
            verify_artifact(&manifest, "нет-такого", b"payload"),
            Err(Error::NoArtifact(_))
        ));
        assert!(manifest.artifact_by_hash(&sha256_hex(b"payload")).is_some());
        assert!(manifest.artifact_by_hash(&sha256_hex(b"other")).is_none());
    }

    #[test]
    fn unsafe_manifest_contents_are_rejected_even_when_signed() {
        let (secret, public) = generate_keypair();
        for bad in [
            r#"{"version":"1.0.0","artifacts":[{"name":"../evil","sha256":"00","size":1}]}"#,
            r#"{"version":"1.0.0","artifacts":[{"name":"a/b","sha256":"0000000000000000000000000000000000000000000000000000000000000000","size":1}]}"#,
            r#"{"version":"latest","artifacts":[]}"#,
        ] {
            let signature = sign(bad.as_bytes(), &secret).unwrap();
            assert!(
                matches!(
                    verify_manifest(bad.as_bytes(), &signature, &public),
                    Err(Error::Manifest(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn sha256_matches_the_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
