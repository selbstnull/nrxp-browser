//! Порядок деплоя и откат.
//!
//! 1. **Probe** — ОС, systemd, права (root или sudo без пароля), архитектура, место, занятость порта; несовместимое — отказ до изменений.
//! 2. Пользователь `nrb-node`, каталоги, заливка бинаря во временный файл → проверка sha256 → **запуск `--version` на сервере**
//!    (ловит «не тот libc» до замены рабочего бинаря) → замена с сохранением предыдущего.
//! 3. Идентичность узла **генерируется на сервере** (`nrb node init`) — секреты по сети не ходят. Существующий узел не пересоздаётся.
//! 4. systemd-юнит, файрвол (ufw), по желанию BBR/fq, запуск, ожидание порта.
//! 5. **Проверка настоящим клиентом**: временное устройство → NRXP-хендшейк и поток с локальной машины → устройство отзывается.
//! 6. Приглашение для пользователя.
//!
//! Любой сбой на шагах 2–5 откатывает всё сделанное: новая установка удаляется целиком, апгрейд возвращает прежний бинарь и юнит.

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::elf::binary_arch;
use crate::remote::{ExecOutput, Remote, sh_quote};
use crate::unit::{BINARY_PATH, DATA_DIR, INSTALL_DIR, SERVICE_NAME, SERVICE_USER, service_unit};
use crate::{DeployError, Result};

const UNIT_PATH: &str = "/etc/systemd/system/nrb-node.service";
const SYSCTL_PATH: &str = "/etc/sysctl.d/99-nrb.conf";

pub struct DeployOptions {
    /// Содержимое бинаря `nrb` для сервера (Linux x86_64/aarch64).
    pub binary: Vec<u8>,
    pub node_port: u16,
    /// Адрес, который попадёт в приглашения (внешний IP или домен сервера).
    pub public_host: String,
    /// Имя первого устройства (его приглашение вернётся в отчёте).
    pub device_name: String,
    pub enable_bbr: bool,
    /// Только проверки сервера и план, без изменений.
    pub dry_run: bool,
    /// Только для тестов отката: после установки считать проверку проваленной.
    pub inject_verify_failure: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Probe {
    pub kernel: String,
    pub os: String,
    pub arch: String,
    pub uid: u32,
    pub sudo: bool,
    pub systemd: bool,
    pub ufw_active: bool,
    pub bbr_available: bool,
    pub port_busy: bool,
    pub installed_version: Option<String>,
    pub service_active: bool,
    pub disk_mb: u64,
    pub node_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct DeployReport {
    /// Приглашение для первого устройства (`nrxp://…`) — **секрет**, показывается один раз.
    pub invite: String,
    pub public_key: String,
    pub upgraded: bool,
    pub warnings: Vec<String>,
    pub probe: Probe,
}

/// Проверка работающего узла настоящим клиентом (локально): хендшейк + поток наружу. Возвращает предупреждения.
#[async_trait]
pub trait Verifier: Send + Sync {
    async fn verify(&self, invite: &str) -> Result<Vec<String>>;
}

fn step(step: &'static str, output: &ExecOutput) -> Result<()> {
    if output.ok() {
        return Ok(());
    }
    let detail = [output.stderr.trim(), output.stdout.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("без вывода");
    Err(DeployError::Step {
        step,
        detail: format!("код {}: {detail}", output.code),
    })
}

struct Session<'a> {
    remote: &'a dyn Remote,
    root: bool,
    progress: &'a dyn Fn(&str),
}

impl Session<'_> {
    /// Команда с правами root (через `sudo -n`, если зашли не root).
    fn privileged(&self, command: &str) -> String {
        if self.root {
            command.to_owned()
        } else {
            format!("sudo -n sh -c {}", sh_quote(command))
        }
    }

    async fn run(&self, name: &'static str, command: &str) -> Result<ExecOutput> {
        let output = self.remote.exec(&self.privileged(command)).await?;
        step(name, &output)?;
        Ok(output)
    }

    async fn try_run(&self, command: &str) -> Result<ExecOutput> {
        self.remote.exec(&self.privileged(command)).await
    }

    /// От имени служебного пользователя узла (там лежат его секреты).
    async fn as_node(&self, name: &'static str, command: &str) -> Result<ExecOutput> {
        self.run(name, &format!("runuser -u {SERVICE_USER} -- {command}"))
            .await
    }

    fn say(&self, message: &str) {
        (self.progress)(message);
    }
}

pub fn parse_probe(text: &str) -> Probe {
    let mut probe = Probe::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "kernel" => probe.kernel = value.to_owned(),
            "os" => probe.os = value.to_owned(),
            "arch" => probe.arch = value.to_owned(),
            "uid" => probe.uid = value.parse().unwrap_or(u32::MAX),
            "sudo" => probe.sudo = value == "yes",
            "systemd" => probe.systemd = value == "yes",
            "ufw" => probe.ufw_active = value == "active",
            "bbr" => probe.bbr_available = value == "yes",
            "port_busy" => probe.port_busy = value == "yes",
            "installed" => {
                probe.installed_version =
                    (value != "none" && !value.is_empty()).then(|| value.to_owned())
            }
            "service_active" => probe.service_active = value == "yes",
            "disk_mb" => probe.disk_mb = value.parse().unwrap_or(0),
            "node_port" => probe.node_port = value.parse().ok(),
            _ => {}
        }
    }
    probe
}

fn probe_script(port: u16) -> String {
    format!(
        "printf 'kernel=%s\\n' \"$(uname -s)\"; printf 'arch=%s\\n' \"$(uname -m)\"; printf 'uid=%s\\n' \"$(id -u)\"; \
         printf 'os=%s\\n' \"$( . /etc/os-release 2>/dev/null; echo \"$PRETTY_NAME\")\"; \
         printf 'systemd=%s\\n' \"$([ -d /run/systemd/system ] && command -v systemctl >/dev/null && echo yes || echo no)\"; \
         printf 'sudo=%s\\n' \"$(sudo -n true 2>/dev/null && echo yes || echo no)\"; \
         printf 'ufw=%s\\n' \"$(command -v ufw >/dev/null 2>&1 && (ufw status 2>/dev/null | head -1 | grep -qi 'status: active' && echo active || echo inactive) || echo none)\"; \
         printf 'bbr=%s\\n' \"$(grep -qw bbr /proc/sys/net/ipv4/tcp_available_congestion_control 2>/dev/null && echo yes || (modprobe -n tcp_bbr >/dev/null 2>&1 && echo yes || echo no))\"; \
         printf 'port_busy=%s\\n' \"$(ss -tlnH 2>/dev/null | awk '{{print $4}}' | grep -qE ':{port}$' && echo yes || echo no)\"; \
         printf 'installed=%s\\n' \"$({BINARY_PATH} --version 2>/dev/null || echo none)\"; \
         printf 'service_active=%s\\n' \"$( (sudo -n systemctl is-active {SERVICE_NAME} 2>/dev/null || systemctl is-active {SERVICE_NAME} 2>/dev/null) | grep -qx active && echo yes || echo no)\"; \
         printf 'disk_mb=%s\\n' \"$(df -Pm /opt 2>/dev/null | awk 'NR==2{{print $4}}')\"; \
         printf 'node_port=%s\\n' \"$(sudo -n grep -E '^port *=' {DATA_DIR}/node.toml 2>/dev/null | head -1 | tr -dc '0-9' || grep -E '^port *=' {DATA_DIR}/node.toml 2>/dev/null | head -1 | tr -dc '0-9')\""
    )
}

/// Подходит ли сервер. `Err` — человекочитаемая причина отказа (до каких-либо изменений).
fn check_suitable(probe: &Probe, opts: &DeployOptions) -> Result<()> {
    let no = |why: String| Err(DeployError::Unsuitable(why));
    if probe.kernel != "Linux" {
        return no(format!(
            "поддерживается только Linux, а тут `{}`",
            probe.kernel
        ));
    }
    if !probe.systemd {
        return no("не найден systemd (нужен для службы узла)".into());
    }
    if probe.uid != 0 && !probe.sudo {
        return no(
            "нужен root или пользователь с `sudo` без пароля (`sudo -n true` не прошёл)".into(),
        );
    }
    let arch = binary_arch(&opts.binary)?;
    if !arch.matches_uname(&probe.arch) {
        return no(format!(
            "сервер `{}`, а бинарь собран для {} — соберите бинарь под архитектуру сервера",
            probe.arch,
            arch.uname()
        ));
    }
    if probe.disk_mb < 150 {
        return no(format!(
            "мало места: свободно {} МБ в /opt (нужно ≥ 150)",
            probe.disk_mb
        ));
    }
    let ours = probe.installed_version.is_some() && probe.service_active;
    if probe.port_busy && !ours {
        return no(format!(
            "порт {} уже занят другим процессом (это не наш узел: установлено={:?}, служба активна={})",
            opts.node_port, probe.installed_version, probe.service_active
        ));
    }
    if let Some(existing) = probe.node_port
        && existing != opts.node_port
    {
        return no(format!(
            "узел уже развёрнут на порту {existing}, а запрошен {} — смена порта на месте не поддерживается (`nrb deploy --remove`, затем заново)",
            opts.node_port
        ));
    }
    Ok(())
}

pub async fn deploy(
    remote: &dyn Remote,
    verifier: &dyn Verifier,
    opts: &DeployOptions,
    progress: &dyn Fn(&str),
) -> Result<DeployReport> {
    // Права определяем по первому вызову: uid из probe, но сам probe уже может требовать sudo — он тихий и безопасный.
    let first = Session {
        remote,
        root: true,
        progress,
    };
    first.say("проверяю сервер…");
    let output = remote.exec(&probe_script(opts.node_port)).await?;
    step("probe", &output)?;
    let probe = parse_probe(&output.stdout);
    check_suitable(&probe, opts)?;
    let session = Session {
        remote,
        root: probe.uid == 0,
        progress,
    };
    let upgrade = probe.installed_version.is_some();
    session.say(&format!(
        "сервер: {} ({}), {}, {}",
        probe.os,
        probe.arch,
        if upgrade {
            format!(
                "уже стоит {} — апгрейд",
                probe.installed_version.as_deref().unwrap_or("?")
            )
        } else {
            "чистая установка".into()
        },
        if probe.uid == 0 { "root" } else { "sudo" }
    ));
    if opts.dry_run {
        session.say("--dry-run: изменений не вношу");
        return Ok(DeployReport {
            invite: String::new(),
            public_key: String::new(),
            upgraded: upgrade,
            warnings: vec!["dry-run: ничего не установлено".into()],
            probe,
        });
    }

    let mut undo = Undo {
        upgrade,
        had_unit: false,
        created_user: false,
        created_data: false,
        opened_ufw: false,
        wrote_sysctl: false,
    };
    match install(&session, &probe, opts, verifier, &mut undo).await {
        Ok(report) => Ok(report),
        Err(error) => {
            session.say(&format!("сбой: {error} — откатываю изменения…"));
            match rollback(&session, &undo, opts).await {
                Ok(()) => session.say("откат выполнен"),
                Err(rollback_error) => session.say(&format!(
                    "ОТКАТ НЕ ЗАВЕРШЁН: {rollback_error} — проверьте сервер вручную"
                )),
            }
            Err(error)
        }
    }
}

struct Undo {
    upgrade: bool,
    had_unit: bool,
    created_user: bool,
    created_data: bool,
    opened_ufw: bool,
    wrote_sysctl: bool,
}

async fn install(
    s: &Session<'_>,
    probe: &Probe,
    opts: &DeployOptions,
    verifier: &dyn Verifier,
    undo: &mut Undo,
) -> Result<DeployReport> {
    let mut warnings = Vec::new();
    let sha = hex(&Sha256::digest(&opts.binary));
    let tmp = format!("/tmp/nrb-deploy-{}.bin", &sha[..12]);

    // --- пользователь и каталоги ---
    s.say("готовлю пользователя и каталоги…");
    let user_exists = s
        .try_run(&format!("getent passwd {SERVICE_USER} >/dev/null"))
        .await?
        .ok();
    if !user_exists {
        s.run("useradd", &format!("useradd --system --user-group --no-create-home --home-dir {DATA_DIR} --shell /usr/sbin/nologin {SERVICE_USER}")).await?;
        undo.created_user = true;
    }
    let data_exists = s.try_run(&format!("[ -d {DATA_DIR} ]")).await?.ok();
    s.run("mkdir", &format!("install -d -m 0755 {INSTALL_DIR} && install -d -m 0700 -o {SERVICE_USER} -g {SERVICE_USER} {DATA_DIR}")).await?;
    undo.created_data = !data_exists;

    // --- заливка и проверка бинаря ---
    s.say(&format!(
        "заливаю бинарь ({:.1} МБ)…",
        opts.binary.len() as f64 / 1_048_576.0
    ));
    s.remote.upload(&tmp, &opts.binary, 0o700).await?;
    let result = verify_and_swap(s, &tmp, &sha).await;
    let _ = s.try_run(&format!("rm -f {}", sh_quote(&tmp))).await;
    result?;

    // --- идентичность (на сервере) ---
    let has_identity = s
        .try_run(&format!("[ -f {DATA_DIR}/node.toml ]"))
        .await?
        .ok();
    if has_identity {
        s.say("узел уже инициализирован — ключи сохраняются");
    } else {
        s.say("создаю идентичность узла на сервере…");
        s.as_node(
            "node init",
            &format!(
                "{BINARY_PATH} node init --dir {DATA_DIR} --public-host {} --port {}",
                sh_quote(&opts.public_host),
                opts.node_port
            ),
        )
        .await?;
    }

    // --- юнит ---
    s.say("настраиваю службу systemd…");
    let wanted = service_unit();
    let old_unit = s.try_run(&format!("cat {UNIT_PATH} 2>/dev/null")).await?;
    undo.had_unit = old_unit.ok();
    if old_unit.stdout != wanted {
        if undo.had_unit {
            s.run(
                "backup unit",
                &format!("cp -a {UNIT_PATH} {UNIT_PATH}.prev"),
            )
            .await?;
        }
        let unit_tmp = format!("/tmp/nrb-unit-{}.service", &sha[..12]);
        s.remote.upload(&unit_tmp, wanted.as_bytes(), 0o644).await?;
        let moved = s
            .run(
                "unit",
                &format!(
                    "install -m 0644 -o root -g root {} {UNIT_PATH}",
                    sh_quote(&unit_tmp)
                ),
            )
            .await;
        let _ = s.try_run(&format!("rm -f {}", sh_quote(&unit_tmp))).await;
        moved?;
    }

    // --- файрвол и сеть ---
    if probe.ufw_active {
        s.say(&format!("открываю порт {} в ufw…", opts.node_port));
        let rule = s
            .run(
                "ufw",
                &format!("ufw allow {p}/tcp && ufw allow {p}/udp", p = opts.node_port),
            )
            .await;
        match rule {
            Ok(_) => undo.opened_ufw = true,
            Err(e) => warnings.push(format!("не удалось открыть порт в ufw: {e}")),
        }
    } else {
        warnings.push(format!("ufw не активен — порт {} не открывался; если у провайдера есть внешний файрвол, откройте TCP+UDP {} там", opts.node_port, opts.node_port));
    }
    if opts.enable_bbr {
        if probe.bbr_available {
            let conf = "net.core.default_qdisc=fq\\nnet.ipv4.tcp_congestion_control=bbr\\n";
            if s.run(
                "sysctl",
                &format!("printf '{conf}' > {SYSCTL_PATH} && sysctl -p {SYSCTL_PATH}"),
            )
            .await
            .is_ok()
            {
                undo.wrote_sysctl = true;
            } else {
                warnings.push("BBR/fq включить не удалось".into());
            }
        } else {
            warnings.push("ядро сервера без BBR — настройка пропущена".into());
        }
    }

    // --- запуск ---
    s.say("запускаю службу…");
    s.run("systemctl", &format!("systemctl daemon-reload && systemctl enable {SERVICE_NAME} >/dev/null 2>&1 && systemctl restart {SERVICE_NAME}")).await?;
    wait_listening(s, opts.node_port).await?;

    // --- проверка настоящим клиентом ---
    s.say("проверяю узел настоящим клиентом (хендшейк и поток)…");
    let check_name = format!("deploy-check-{}", &sha[..6]);
    let check_link = invite(s, &check_name).await?;
    let verified = if opts.inject_verify_failure {
        Err(DeployError::Verify(
            "искусственный сбой проверки (тест отката)".into(),
        ))
    } else {
        verifier.verify(&check_link).await
    };
    let _ = s
        .as_node(
            "revoke",
            &format!(
                "{BINARY_PATH} node revoke --dir {DATA_DIR} {}",
                sh_quote(&check_name)
            ),
        )
        .await;
    warnings.extend(verified?);

    // --- приглашение пользователю ---
    let link = invite(s, &opts.device_name).await?;
    let public_key = nrb_node::Invite::parse(&link)
        .map_err(|e| DeployError::Step {
            step: "invite",
            detail: e.to_string(),
        })?
        .public_key;
    s.say("готово");
    Ok(DeployReport {
        invite: link,
        public_key,
        upgraded: probe.installed_version.is_some(),
        warnings,
        probe: probe.clone(),
    })
}

async fn invite(s: &Session<'_>, name: &str) -> Result<String> {
    let output = s
        .as_node(
            "node invite",
            &format!(
                "{BINARY_PATH} node invite --dir {DATA_DIR} {}",
                sh_quote(name)
            ),
        )
        .await?;
    output
        .stdout
        .lines()
        .find(|line| line.starts_with("nrxp://"))
        .map(str::to_owned)
        .ok_or_else(|| DeployError::Step {
            step: "node invite",
            detail: "узел не вернул ссылку-приглашение".into(),
        })
}

async fn verify_and_swap(s: &Session<'_>, tmp: &str, expected_sha: &str) -> Result<()> {
    let tmp_q = sh_quote(tmp);
    let remote_sha = s
        .run("sha256", &format!("sha256sum {tmp_q} | cut -d' ' -f1"))
        .await?
        .stdout
        .trim()
        .to_owned();
    if remote_sha != expected_sha {
        return Err(DeployError::Step {
            step: "sha256",
            detail: format!(
                "после заливки контрольная сумма не совпала (на сервере {remote_sha}) — файл повреждён по пути"
            ),
        });
    }
    s.run(
        "install",
        &format!("install -m 0755 -o root -g root {tmp_q} {BINARY_PATH}.new"),
    )
    .await?;
    // Запуск на сервере ДО замены рабочего бинаря: несовместимый libc/архитектура вылезут здесь, а не после остановки службы.
    let version = s.try_run(&format!("{BINARY_PATH}.new --version")).await?;
    if !version.ok() || !version.stdout.starts_with("nrb ") {
        let _ = s.try_run(&format!("rm -f {BINARY_PATH}.new")).await;
        return Err(DeployError::Step {
            step: "запуск бинаря на сервере",
            detail: format!(
                "`nrb --version` не отработал (код {}): {} — вероятно, бинарь динамический и не подходит этому дистрибутиву; соберите статический musl-бинарь",
                version.code,
                version.stderr.trim()
            ),
        });
    }
    s.run(
        "swap",
        &format!("[ -f {BINARY_PATH} ] && cp -a {BINARY_PATH} {BINARY_PATH}.prev; mv -f {BINARY_PATH}.new {BINARY_PATH}"),
    )
    .await?;
    Ok(())
}

async fn wait_listening(s: &Session<'_>, port: u16) -> Result<()> {
    for _ in 0..20 {
        if s.try_run(&format!(
            "ss -tlnH | awk '{{print $4}}' | grep -qE ':{port}$'"
        ))
        .await?
        .ok()
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    }
    let log = s
        .try_run(&format!(
            "journalctl -u {SERVICE_NAME} -n 15 --no-pager 2>&1 | tail -15"
        ))
        .await
        .map(|o| o.stdout)
        .unwrap_or_default();
    Err(DeployError::Step {
        step: "ожидание порта",
        detail: format!("служба не начала слушать порт {port} за 15 с. Журнал:\n{log}"),
    })
}

async fn rollback(s: &Session<'_>, undo: &Undo, opts: &DeployOptions) -> Result<()> {
    if undo.upgrade {
        // Возвращаем прежние бинарь и юнит, перезапускаем.
        s.try_run(&format!("[ -f {BINARY_PATH}.prev ] && mv -f {BINARY_PATH}.prev {BINARY_PATH}; rm -f {BINARY_PATH}.new")).await?;
        if undo.had_unit {
            s.try_run(&format!(
                "[ -f {UNIT_PATH}.prev ] && mv -f {UNIT_PATH}.prev {UNIT_PATH}"
            ))
            .await?;
        }
        s.try_run(&format!(
            "systemctl daemon-reload; systemctl restart {SERVICE_NAME}"
        ))
        .await?;
        return Ok(());
    }
    // Чистая установка: убираем всё созданное.
    s.try_run(&format!("systemctl disable --now {SERVICE_NAME} >/dev/null 2>&1; rm -f {UNIT_PATH} {UNIT_PATH}.prev; systemctl daemon-reload")).await?;
    s.try_run(&format!("rm -rf {INSTALL_DIR}")).await?;
    if undo.created_data {
        s.try_run(&format!("rm -rf {DATA_DIR}")).await?;
    }
    if undo.created_user {
        s.try_run(&format!(
            "userdel {SERVICE_USER} >/dev/null 2>&1; groupdel {SERVICE_USER} >/dev/null 2>&1; true"
        ))
        .await?;
    }
    if undo.opened_ufw {
        s.try_run(&format!("ufw delete allow {p}/tcp >/dev/null 2>&1; ufw delete allow {p}/udp >/dev/null 2>&1; true", p = opts.node_port)).await?;
    }
    if undo.wrote_sysctl {
        s.try_run(&format!("rm -f {SYSCTL_PATH}")).await?;
    }
    Ok(())
}

/// Полное удаление узла с сервера (служба, бинарь, данные, пользователь, правило ufw).
pub async fn remove(remote: &dyn Remote, node_port: u16, progress: &dyn Fn(&str)) -> Result<()> {
    let output = remote.exec("id -u").await?;
    step("id", &output)?;
    let s = Session {
        remote,
        root: output.stdout.trim() == "0",
        progress,
    };
    s.say("останавливаю службу и удаляю узел…");
    s.run("remove", &format!(
        "systemctl disable --now {SERVICE_NAME} >/dev/null 2>&1; rm -f {UNIT_PATH} {UNIT_PATH}.prev {SYSCTL_PATH}; systemctl daemon-reload; \
         rm -rf {INSTALL_DIR} {DATA_DIR}; userdel {SERVICE_USER} >/dev/null 2>&1; groupdel {SERVICE_USER} >/dev/null 2>&1; \
         command -v ufw >/dev/null 2>&1 && {{ ufw delete allow {node_port}/tcp >/dev/null 2>&1; ufw delete allow {node_port}/udp >/dev/null 2>&1; }}; true"
    ))
    .await?;
    s.say("удалено");
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
