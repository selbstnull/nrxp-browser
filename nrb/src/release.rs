//! Релизы и обновления с подписью (I14): `nrb release keygen|manifest|sign|verify` (для выпускающего) и
//! `nrb update check|download` (для пользователя). Подпись — ed25519 (`nrb-update`): сначала проверяется подпись манифеста
//! публичным ключом, потом версия (откат запрещён) и хеш каждого скачанного артефакта.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Subcommand;
use nrb_net::Fetcher;
use nrb_transport::{NodeConfig, NrxpTransport, SystemTransport, Transport};
use nrb_update::{Artifact, Manifest};

/// Публичный ключ канала обновлений, зашитый при сборке (`NRB_UPDATE_PUBKEY=<hex> cargo build …`). Без него ключ задают `--pubkey`.
pub fn default_pubkey() -> Option<&'static str> {
    option_env!("NRB_UPDATE_PUBKEY")
}

#[derive(Subcommand)]
pub enum ReleaseCommand {
    /// Создать пару ключей подписи релизов: `update.key` (СЕКРЕТ, 0600) и `update.pub`.
    Keygen {
        /// Каталог для ключей.
        #[arg(long)]
        out: PathBuf,
    },
    /// Собрать манифест релиза (версия, артефакты с SHA-256 и размером).
    Manifest {
        #[arg(long)]
        version: String,
        #[arg(long, default_value = "stable")]
        channel: String,
        #[arg(long, default_value = "")]
        notes: String,
        /// Файлы-артефакты (можно несколько).
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<PathBuf>,
        /// Базовый адрес для скачивания: к нему добавляется имя файла.
        #[arg(long, default_value = "")]
        base_url: String,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Подписать манифест (подпись — рядом, `<манифест>.sig`).
    Sign {
        manifest: PathBuf,
        #[arg(long)]
        key: PathBuf,
    },
    /// Проверить подпись манифеста и (по желанию) файлы артефактов в каталоге.
    Verify {
        manifest: PathBuf,
        #[arg(long)]
        pubkey: Option<PathBuf>,
        /// Подпись; по умолчанию `<манифест>.sig`.
        #[arg(long)]
        signature: Option<PathBuf>,
        /// Каталог с артефактами для проверки хешей.
        #[arg(long)]
        artifacts_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum UpdateCommand {
    /// Есть ли новая версия: скачать манифест и подпись (`<адрес>.sig`), проверить, сравнить с установленной.
    Check {
        /// Адрес манифеста (HTTPS).
        #[arg(long)]
        url: String,
        #[arg(long)]
        pubkey: Option<PathBuf>,
        /// Загружать через узел NRXP (конфиг или `nrxp://…`); без него — напрямую.
        #[arg(long)]
        node: Option<PathBuf>,
        /// Принять и более старую версию (откат).
        #[arg(long)]
        allow_downgrade: bool,
    },
    /// Скачать артефакт из проверенного манифеста в каталог (хеш проверяется до записи).
    Download {
        #[arg(long)]
        url: String,
        #[arg(long)]
        pubkey: Option<PathBuf>,
        #[arg(long)]
        node: Option<PathBuf>,
        /// Имя артефакта; по умолчанию — первый, имя которого содержит `linux-<архитектура>`.
        #[arg(long)]
        artifact: Option<String>,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        allow_downgrade: bool,
    },
}

fn read_trim(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn load_pubkey(path: Option<&Path>) -> Result<String, String> {
    match (path, default_pubkey()) {
        (Some(path), _) => read_trim(path),
        (None, Some(key)) => Ok(key.to_owned()),
        (None, None) => {
            Err("нужен публичный ключ: --pubkey <файл> (в сборку ключ не зашит)".into())
        }
    }
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn run_release(command: ReleaseCommand) -> i32 {
    match release(command) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn release(command: ReleaseCommand) -> Result<(), String> {
    match command {
        ReleaseCommand::Keygen { out } => {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            let (secret, public) = nrb_update::generate_keypair();
            write_private(&out.join("update.key"), &format!("{secret}\n"))?;
            std::fs::write(out.join("update.pub"), format!("{public}\n"))
                .map_err(|e| e.to_string())?;
            println!(
                "ключи в {}:\n  update.pub (публичный) — раздавать и зашивать в сборку (NRB_UPDATE_PUBKEY)\n  update.key (СЕКРЕТНЫЙ, 0600) — хранить только у выпускающего, не коммитить",
                out.display()
            );
            println!("публичный ключ: {public}");
            Ok(())
        }
        ReleaseCommand::Manifest {
            version,
            channel,
            notes,
            artifacts,
            base_url,
            output,
        } => {
            let mut items = Vec::new();
            for path in &artifacts {
                let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or("некорректное имя файла")?
                    .to_owned();
                items.push(Artifact {
                    url: if base_url.is_empty() {
                        String::new()
                    } else {
                        format!("{}/{name}", base_url.trim_end_matches('/'))
                    },
                    sha256: nrb_update::sha256_hex(&data),
                    size: data.len() as u64,
                    name,
                });
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let bytes = nrb_update::build_manifest(&version, &channel, &notes, now, items)
                .map_err(|e| e.to_string())?;
            std::fs::write(&output, bytes).map_err(|e| e.to_string())?;
            println!(
                "манифест: {} (версия {version}, артефактов: {}). Подпишите: nrb release sign {} --key update.key",
                output.display(),
                artifacts.len(),
                output.display()
            );
            Ok(())
        }
        ReleaseCommand::Sign { manifest, key } => {
            let bytes =
                std::fs::read(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
            let signature =
                nrb_update::sign(&bytes, &read_trim(&key)?).map_err(|e| e.to_string())?;
            let sig_path = sig_path(&manifest);
            std::fs::write(&sig_path, format!("{signature}\n")).map_err(|e| e.to_string())?;
            println!("подпись: {}", sig_path.display());
            Ok(())
        }
        ReleaseCommand::Verify {
            manifest,
            pubkey,
            signature,
            artifacts_dir,
        } => {
            let bytes =
                std::fs::read(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
            let signature = read_trim(&signature.unwrap_or_else(|| sig_path(&manifest)))?;
            let parsed =
                nrb_update::verify_manifest(&bytes, &signature, &load_pubkey(pubkey.as_deref())?)
                    .map_err(|e| e.to_string())?;
            println!(
                "подпись верна. Версия {}, артефактов: {}",
                parsed.version,
                parsed.artifacts.len()
            );
            if let Some(dir) = artifacts_dir {
                for artifact in &parsed.artifacts {
                    let data = std::fs::read(dir.join(&artifact.name))
                        .map_err(|e| format!("{}: {e}", artifact.name))?;
                    nrb_update::verify_artifact(&parsed, &artifact.name, &data)
                        .map_err(|e| e.to_string())?;
                    println!("  ok  {}", artifact.name);
                }
            }
            Ok(())
        }
    }
}

fn sig_path(manifest: &Path) -> PathBuf {
    let mut name = manifest.as_os_str().to_owned();
    name.push(".sig");
    PathBuf::from(name)
}

fn transport(
    node: &Option<PathBuf>,
) -> Result<(Arc<dyn Transport>, Option<tokio::runtime::Handle>), String> {
    let Some(link) = node else {
        return Ok((Arc::new(SystemTransport), None));
    };
    let config = if link.to_string_lossy().starts_with("nrxp://") {
        nrb_node::Invite::parse(&link.to_string_lossy())
            .map_err(|e| format!("приглашение: {e}"))?
            .to_node_config()
    } else {
        NodeConfig::from_toml_file(link).map_err(|e| format!("{}: {e}", link.display()))?
    };
    let runtime = tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?;
    let transport = futures_block(async { NrxpTransport::connect(&config).await })
        .map_err(|e| format!("подключение к узлу: {e}"))?;
    Ok((Arc::new(transport), Some(runtime)))
}

/// Мини-обёртка: внутри уже запущенного рантайма (`block_in_place`).
fn futures_block<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

pub fn run_update(command: UpdateCommand) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 3;
        }
    };
    match runtime.block_on(update(command)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

async fn fetch_verified(
    url: &str,
    pubkey: Option<&Path>,
    node: &Option<PathBuf>,
    allow_downgrade: bool,
) -> Result<(Manifest, Fetcher), String> {
    let key = load_pubkey(pubkey)?;
    let (transport, _) = transport(node)?;
    let fetcher = Fetcher::new(transport).with_timeout(Duration::from_secs(120));
    let manifest_url: url::Url = url.parse().map_err(|e| format!("адрес манифеста: {e}"))?;
    let sig_url: url::Url = format!("{url}.sig")
        .parse()
        .map_err(|e| format!("адрес подписи: {e}"))?;
    let manifest = fetcher
        .get(&manifest_url)
        .await
        .map_err(|e| format!("манифест: {e}"))?;
    let signature = fetcher
        .get(&sig_url)
        .await
        .map_err(|e| format!("подпись: {e}"))?;
    if manifest.status != 200 || signature.status != 200 {
        return Err(format!(
            "сервер ответил {} / {}",
            manifest.status, signature.status
        ));
    }
    let signature = String::from_utf8_lossy(&signature.body).trim().to_owned();
    let parsed =
        nrb_update::verify_manifest(&manifest.body, &signature, &key).map_err(|e| e.to_string())?;
    nrb_update::check_not_downgrade(&parsed, env!("CARGO_PKG_VERSION"), allow_downgrade)
        .map_err(|e| e.to_string())?;
    Ok((parsed, fetcher))
}

async fn update(command: UpdateCommand) -> Result<(), String> {
    match command {
        UpdateCommand::Check {
            url,
            pubkey,
            node,
            allow_downgrade,
        } => match fetch_verified(&url, pubkey.as_deref(), &node, allow_downgrade).await {
            Ok((manifest, _)) => {
                println!(
                    "доступна версия {} (установлена {}), подпись верна.",
                    manifest.version,
                    env!("CARGO_PKG_VERSION")
                );
                if !manifest.notes.is_empty() {
                    println!("{}", manifest.notes);
                }
                for a in &manifest.artifacts {
                    println!("  {}  {} байт  sha256 {}", a.name, a.size, a.sha256);
                }
                Ok(())
            }
            Err(error) if error.contains("откат запрещён") => {
                println!(
                    "установлена самая новая версия ({}).",
                    env!("CARGO_PKG_VERSION")
                );
                Ok(())
            }
            Err(error) => Err(error),
        },
        UpdateCommand::Download {
            url,
            pubkey,
            node,
            artifact,
            out,
            allow_downgrade,
        } => {
            let (manifest, fetcher) =
                fetch_verified(&url, pubkey.as_deref(), &node, allow_downgrade).await?;
            let want = std::env::consts::ARCH;
            let item = match &artifact {
                Some(name) => manifest.artifact(name).map_err(|e| e.to_string())?,
                None => manifest
                    .artifacts
                    .iter()
                    .find(|a| a.name.contains(&format!("linux-{want}")))
                    .ok_or_else(|| {
                        format!("в манифесте нет артефакта для linux-{want}; укажите --artifact")
                    })?,
            };
            if item.url.is_empty() {
                return Err("у артефакта нет адреса (url) в манифесте".into());
            }
            let artifact_url: url::Url = item
                .url
                .parse()
                .map_err(|e| format!("адрес артефакта: {e}"))?;
            let response = fetcher
                .get(&artifact_url)
                .await
                .map_err(|e| format!("загрузка: {e}"))?;
            if response.status != 200 {
                return Err(format!("сервер ответил {}", response.status));
            }
            // Хеш проверяется ДО записи на диск.
            nrb_update::verify_artifact(&manifest, &item.name, &response.body)
                .map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            let path = out.join(&item.name);
            std::fs::write(&path, &response.body).map_err(|e| e.to_string())?;
            println!(
                "скачано и проверено (sha256 совпал): {} ({} байт). Версия {}.",
                path.display(),
                response.body.len(),
                manifest.version
            );
            Ok(())
        }
    }
}
