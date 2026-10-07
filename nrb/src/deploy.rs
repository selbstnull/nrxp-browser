//! `nrb deploy user@host` (I8): поставить узел на свой сервер по SSH и получить приглашение.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use clap::Args;
use nrb_deploy::{DeployOptions, HostKeyPolicy, KnownHosts, NrxpVerifier, SshAuth, SshRemote};

#[derive(Args)]
pub struct DeployArgs {
    /// `пользователь@адрес` для SSH (пользователь — root или с `sudo` без пароля).
    pub target: String,
    #[arg(long, default_value_t = 22)]
    pub ssh_port: u16,
    /// Приватный ключ SSH (по умолчанию ~/.ssh/id_ed25519, затем id_rsa; иначе — пароль).
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Имя переменной окружения с паролем SSH (иначе — запрос без эха). Пароль используется только для этого сеанса.
    #[arg(long, value_name = "ПЕРЕМЕННАЯ")]
    pub password_env: Option<String>,
    /// Порт узла на сервере (TCP+UDP).
    #[arg(long, default_value_t = 443)]
    pub node_port: u16,
    /// Адрес для приглашений (внешний IP/домен); по умолчанию — адрес из `target`.
    #[arg(long)]
    pub public_host: Option<String>,
    /// Бинарь `nrb` для сервера (Linux x86_64/aarch64), либо `self` — текущий исполняемый файл. Для старых дистрибутивов нужна статическая musl-сборка.
    #[arg(long)]
    pub binary: Option<String>,
    /// Имя первого устройства, для которого выдаётся приглашение.
    #[arg(long, default_value = "первое-устройство")]
    pub device: String,
    /// Отпечаток SSH-ключа сервера (`SHA256:…`), которому доверяем без вопросов (для скриптов). Без него — пин или запрос.
    #[arg(long, value_name = "ОТПЕЧАТОК")]
    pub trust_host_key: Option<String>,
    /// Включить BBR и fq на сервере (если ядро поддерживает).
    #[arg(long)]
    pub bbr: bool,
    /// Только проверить сервер и показать план.
    #[arg(long)]
    pub dry_run: bool,
    /// Удалить узел с сервера (служба, бинарь, данные, пользователь).
    #[arg(long, conflicts_with = "binary")]
    pub remove: bool,
    /// Служебный флаг тестов отката: считать проверку после установки проваленной.
    #[arg(long, hide = true)]
    pub inject_verify_failure: bool,
}

fn data_dir() -> Option<PathBuf> {
    crate::node::default_data_dir()
}

fn split_target(target: &str) -> Result<(String, String), String> {
    let (user, host) = target
        .split_once('@')
        .ok_or("ожидается `пользователь@адрес`")?;
    if user.is_empty() || host.is_empty() {
        return Err("ожидается `пользователь@адрес`".into());
    }
    Ok((user.to_owned(), host.to_owned()))
}

fn default_key() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    ["id_ed25519", "id_rsa"]
        .iter()
        .map(|name| home.join(".ssh").join(name))
        .find(|p| p.exists())
}

fn auth(args: &DeployArgs) -> Result<SshAuth, String> {
    if let Some(var) = &args.password_env {
        return std::env::var(var)
            .map(SshAuth::Password)
            .map_err(|_| format!("переменная {var} не задана"));
    }
    if let Some(path) = args.key.clone().or_else(default_key) {
        let passphrase = std::env::var("NRB_SSH_KEY_PASSPHRASE").ok();
        return Ok(SshAuth::Key { path, passphrase });
    }
    if !std::io::stdin().is_terminal() {
        return Err(
            "нет ключа SSH (--key) и некуда запросить пароль: задайте --key или --password-env"
                .into(),
        );
    }
    rpassword::prompt_password("Пароль SSH: ")
        .map(SshAuth::Password)
        .map_err(|e| e.to_string())
}

/// Решение по хост-ключу: пин из файла или `--trust-host-key`; иначе спросить (в интерактивном терминале).
fn host_key_policy(args: &DeployArgs, host: &str, known: &KnownHosts) -> HostKeyPolicy {
    if let Some(fp) = &args.trust_host_key {
        return HostKeyPolicy::Pinned(fp.clone());
    }
    if let Some(fp) = known.get(host, args.ssh_port) {
        return HostKeyPolicy::Pinned(fp.to_owned());
    }
    let host = host.to_owned();
    HostKeyPolicy::Ask(Box::new(move |fingerprint| {
        eprintln!(
            "\nСервер {host} раньше не встречался. Отпечаток его SSH-ключа:\n  {fingerprint}"
        );
        eprintln!(
            "Сверьте с отпечатком в панели хостинга/консоли сервера (`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`)."
        );
        if !std::io::stdin().is_terminal() {
            eprintln!(
                "Не интерактивный запуск: отпечаток не принят. Повторите с `--trust-host-key {fingerprint}` после сверки."
            );
            return false;
        }
        eprint!("Доверять этому серверу и запомнить отпечаток? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).is_ok()
            && matches!(
                answer.trim().to_lowercase().as_str(),
                "y" | "yes" | "д" | "да"
            )
    }))
}

pub fn run(args: DeployArgs) -> i32 {
    match execute(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn execute(args: DeployArgs) -> Result<(), String> {
    let (user, host) = split_target(&args.target)?;
    let binary = if args.remove {
        Vec::new()
    } else {
        let spec = args
            .binary
            .as_deref()
            .ok_or("укажите --binary <путь к nrb для сервера> (или `self`)")?;
        let path = if spec == "self" {
            std::env::current_exe().map_err(|e| e.to_string())?
        } else {
            PathBuf::from(spec)
        };
        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?
    };
    let known_path = data_dir()
        .map(|d| d.join("known_hosts.toml"))
        .ok_or("не удалось определить каталог данных")?;
    let mut known = KnownHosts::load(&known_path).map_err(|e| e.to_string())?;
    let auth = auth(&args)?;
    let policy = host_key_policy(&args, &host, &known);
    // Пин сохраняется и при явном `--trust-host-key`: следующие запуски идут без флага. Уже сохранённый такой же — не пишем заново.
    let already_pinned = known.get(&host, args.ssh_port).map(str::to_owned);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        eprintln!("подключаюсь к {user}@{host}:{}…", args.ssh_port);
        let remote = SshRemote::connect(&host, args.ssh_port, &user, auth, policy)
            .await
            .map_err(|e| e.to_string())?;
        if already_pinned.as_deref() != Some(remote.fingerprint.as_str()) {
            known
                .pin(&host, args.ssh_port, &remote.fingerprint)
                .map_err(|e| e.to_string())?;
            eprintln!("отпечаток запомнен ({})", known_path.display());
        }
        let progress = |message: &str| eprintln!("  • {message}");
        let result = if args.remove {
            nrb_deploy::remove(&remote, args.node_port, &progress)
                .await
                .map(|()| None)
        } else {
            let options = DeployOptions {
                binary,
                node_port: args.node_port,
                public_host: args.public_host.clone().unwrap_or_else(|| host.clone()),
                device_name: args.device.clone(),
                enable_bbr: args.bbr,
                dry_run: args.dry_run,
                inject_verify_failure: args.inject_verify_failure,
            };
            nrb_deploy::deploy(
                &remote,
                &NrxpVerifier {
                    require_egress: false,
                },
                &options,
                &progress,
            )
            .await
            .map(Some)
        };
        remote.close().await;
        match result {
            Err(error) => Err(error.to_string()),
            Ok(None) => Ok(()),
            Ok(Some(report)) => {
                for warning in &report.warnings {
                    eprintln!("  ⚠ {warning}");
                }
                if report.invite.is_empty() {
                    return Ok(());
                }
                eprintln!(
                    "\nУзел {} на {host}. Публичный ключ: {}",
                    if report.upgraded {
                        "обновлён"
                    } else {
                        "установлен"
                    },
                    report.public_key
                );
                eprintln!(
                    "Приглашение для «{}» (СЕКРЕТ, токен показан один раз):\n",
                    args.device
                );
                println!("{}", report.invite);
                eprintln!(
                    "\nНа устройстве: `nrb pair '<ссылка>'` или `nrb --node '<ссылка>' <адрес>`."
                );
                Ok(())
            }
        }
    })
}
