//! SSH-клиент на `russh`: выполнение команд и заливка файлов. Системный `ssh` не используется.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh::{ChannelMsg, Disconnect};
use tokio::io::AsyncWriteExt;

use crate::remote::{ExecOutput, Remote, sh_quote};
use crate::{DeployError, Result};

/// Что делать с хост-ключом сервера. Молча принимать нельзя ни при каких условиях.
pub enum HostKeyPolicy {
    /// Ожидаемый отпечаток (`SHA256:…`) уже известен: из пина или из `--trust-host-key`.
    Pinned(String),
    /// Отпечатка нет — спросить пользователя; колбэк получает отпечаток и возвращает решение.
    Ask(Box<dyn Fn(&str) -> bool + Send + Sync>),
}

pub enum SshAuth {
    Key {
        path: PathBuf,
        passphrase: Option<String>,
    },
    Password(String),
}

struct Client {
    policy: HostKeyPolicy,
    seen: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            // Сертификаты серверов не поддерживаем: пока нечем проверить доверие к CA.
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        *self.seen.lock().expect("mutex") = Some(fingerprint.clone());
        Ok(match &self.policy {
            HostKeyPolicy::Pinned(expected) => *expected == fingerprint,
            HostKeyPolicy::Ask(ask) => ask(&fingerprint),
        })
    }
}

pub struct SshRemote {
    handle: client::Handle<Client>,
    /// Отпечаток хост-ключа, с которым установлено соединение.
    pub fingerprint: String,
}

fn ssh_error(error: impl std::fmt::Display) -> DeployError {
    DeployError::Ssh(error.to_string())
}

impl SshRemote {
    pub async fn connect(
        host: &str,
        port: u16,
        user: &str,
        auth: SshAuth,
        policy: HostKeyPolicy,
    ) -> Result<Self> {
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(180)),
            keepalive_interval: Some(Duration::from_secs(15)),
            ..Default::default()
        });
        let seen = Arc::new(Mutex::new(None));
        let pinned = matches!(policy, HostKeyPolicy::Pinned(_));
        let handler = Client {
            policy,
            seen: seen.clone(),
        };
        let connect = tokio::time::timeout(
            Duration::from_secs(20),
            client::connect(config, (host, port), handler),
        )
        .await
        .map_err(|_| DeployError::Ssh(format!("{host}:{port} не отвечает за 20 с")))?;
        let mut handle = match connect {
            Ok(handle) => handle,
            Err(error) => {
                let fingerprint = seen.lock().expect("mutex").clone();
                return Err(match fingerprint {
                    Some(fp) if pinned => DeployError::HostKey(format!(
                        "отпечаток {fp} не совпал с сохранённым — возможна подмена сервера (MITM) или он переустановлен; если это ожидаемо, удалите пин из known_hosts.toml"
                    )),
                    Some(fp) => DeployError::HostKey(format!("отпечаток {fp} не принят")),
                    None => ssh_error(error),
                });
            }
        };
        let fingerprint = seen
            .lock()
            .expect("mutex")
            .clone()
            .ok_or_else(|| DeployError::HostKey("сервер не предъявил ключ".into()))?;

        let authenticated = match auth {
            SshAuth::Key { path, passphrase } => {
                let key = load_secret_key(&path, passphrase.as_deref())
                    .map_err(|e| DeployError::Ssh(format!("ключ {}: {e}", path.display())))?;
                let hash = handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(ssh_error)?
                    .flatten();
                handle
                    .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                    .await
                    .map_err(ssh_error)?
            }
            SshAuth::Password(password) => handle
                .authenticate_password(user, password)
                .await
                .map_err(ssh_error)?,
        };
        if !authenticated.success() {
            return Err(DeployError::Ssh(format!(
                "сервер отклонил вход пользователя {user} (неверный ключ или пароль)"
            )));
        }
        Ok(Self {
            handle,
            fingerprint,
        })
    }

    pub async fn close(&self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "ru")
            .await;
    }
}

#[async_trait]
impl Remote for SshRemote {
    async fn exec(&self, command: &str) -> Result<ExecOutput> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(ssh_error)?;
        channel.exec(true, command).await.map_err(ssh_error)?;
        collect(&mut channel).await
    }

    async fn upload(&self, remote_path: &str, data: &[u8], mode: u32) -> Result<()> {
        let path = sh_quote(remote_path);
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(ssh_error)?;
        channel
            .exec(
                true,
                format!("umask 077 && cat > {path} && chmod {mode:o} {path}"),
            )
            .await
            .map_err(ssh_error)?;
        let mut writer = channel.make_writer();
        writer.write_all(data).await.map_err(ssh_error)?;
        writer.flush().await.map_err(ssh_error)?;
        drop(writer);
        channel.eof().await.map_err(ssh_error)?;
        let output = collect(&mut channel).await?;
        if output.ok() {
            Ok(())
        } else {
            Err(DeployError::Ssh(format!(
                "заливка {remote_path}: код {} {}",
                output.code,
                output.stderr.trim()
            )))
        }
    }
}

async fn collect(channel: &mut russh::Channel<client::Msg>) -> Result<ExecOutput> {
    let (mut stdout, mut stderr, mut code) = (Vec::new(), Vec::new(), None);
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
            _ => {}
        }
    }
    Ok(ExecOutput {
        code: code.ok_or_else(|| {
            DeployError::Ssh("команда завершилась без кода выхода (обрыв соединения?)".into())
        })?,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}
