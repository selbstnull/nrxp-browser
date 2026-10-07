use std::sync::Mutex;

use async_trait::async_trait;

use super::*;

/// Поддельный сервер: команды сопоставляются с правилами по подстроке (первое совпадение), всё остальное — успех без вывода.
struct Fake {
    rules: Mutex<Vec<(String, ExecOutput)>>,
    log: Mutex<Vec<String>>,
    uploads: Mutex<Vec<String>>,
}

impl Fake {
    fn new(rules: Vec<(&str, ExecOutput)>) -> Self {
        Self {
            rules: Mutex::new(rules.into_iter().map(|(p, o)| (p.to_owned(), o)).collect()),
            log: Mutex::default(),
            uploads: Mutex::default(),
        }
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn ran(&self, needle: &str) -> bool {
        self.log().iter().any(|c| c.contains(needle))
    }
}

#[async_trait]
impl Remote for Fake {
    async fn exec(&self, command: &str) -> Result<ExecOutput> {
        self.log.lock().unwrap().push(command.to_owned());
        let rules = self.rules.lock().unwrap();
        Ok(rules
            .iter()
            .find(|(pattern, _)| command.contains(pattern.as_str()))
            .map(|(_, o)| o.clone())
            .unwrap_or_default())
    }
    async fn upload(&self, remote_path: &str, _data: &[u8], _mode: u32) -> Result<()> {
        self.uploads.lock().unwrap().push(remote_path.to_owned());
        Ok(())
    }
}

struct FakeVerifier(std::result::Result<Vec<String>, String>);

#[async_trait]
impl Verifier for FakeVerifier {
    async fn verify(&self, _invite: &str) -> Result<Vec<String>> {
        self.0.clone().map_err(DeployError::Verify)
    }
}

fn elf_x86() -> Vec<u8> {
    let mut data = vec![0u8; 4096];
    data[..4].copy_from_slice(b"\x7fELF");
    data[4] = 2;
    data[5] = 1;
    data[18..20].copy_from_slice(&62u16.to_le_bytes());
    data
}

fn options() -> DeployOptions {
    DeployOptions {
        binary: elf_x86(),
        node_port: 443,
        public_host: "203.0.113.7".into(),
        device_name: "телефон".into(),
        enable_bbr: false,
        dry_run: false,
        inject_verify_failure: false,
    }
}

fn out(code: u32, stdout: &str) -> ExecOutput {
    ExecOutput {
        code,
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

fn probe_text(extra: &str) -> String {
    format!(
        "kernel=Linux\narch=x86_64\nuid=0\nos=Ubuntu 24.04\nsystemd=yes\nsudo=no\nufw=inactive\nbbr=yes\nport_busy=no\ninstalled=none\nservice_active=no\ndisk_mb=9000\nnode_port=\n{extra}"
    )
}

fn invite_link(name: &str) -> String {
    nrb_node::Invite {
        host: "203.0.113.7".into(),
        port: 443,
        public_key: "ab".repeat(32),
        secret: "cd".repeat(32),
        token: "ef".repeat(32),
        decoy_sni: "www.debian.org".into(),
        name: name.into(),
    }
    .to_url()
}

fn sha_of(binary: &[u8]) -> String {
    hex(&Sha256::digest(binary))
}

/// Правила «здоровой» чистой установки; `probe` подставляется вызывающим.
fn happy_rules(probe: String) -> Vec<(&'static str, ExecOutput)> {
    let sha = sha_of(&elf_x86());
    vec![
        ("printf 'kernel=", out(0, &probe)),
        ("getent passwd", out(2, "")),
        ("[ -d /var/lib/nrb-node ]", out(1, "")),
        ("sha256sum", out(0, &format!("{sha}\n"))),
        (".new --version", out(0, "nrb 0.0.1\n")),
        ("[ -f /var/lib/nrb-node/node.toml ]", out(1, "")),
        ("cat /etc/systemd/system/nrb-node.service", out(1, "")),
        ("grep -qE", out(0, "")),
        ("node invite", out(0, &format!("{}\n", invite_link("x")))),
    ]
}

async fn run(fake: &Fake, verifier: &FakeVerifier, opts: &DeployOptions) -> Result<DeployReport> {
    deploy(fake, verifier, opts, &|_| {}).await
}

#[tokio::test]
async fn fresh_install_goes_through_all_steps_in_order() {
    let fake = Fake::new(happy_rules(probe_text("")));
    let report = run(&fake, &FakeVerifier(Ok(vec![])), &options())
        .await
        .unwrap();
    assert!(!report.upgraded);
    assert_eq!(report.public_key, "ab".repeat(32));
    assert!(report.invite.starts_with("nrxp://203.0.113.7:443"));

    let log = fake.log().join("\n");
    let order = [
        "useradd",
        "install -d -m 0755 /opt/nrb",
        "sha256sum",
        ".new --version",
        "mv -f",
        "node init",
        "systemctl daemon-reload",
        "node invite",
        "node revoke",
    ];
    let mut last = 0;
    for needle in order {
        let at = log[last..]
            .find(needle)
            .unwrap_or_else(|| panic!("нет шага «{needle}» после предыдущего. Журнал:\n{log}"));
        last += at;
    }
    assert!(
        !fake.ran("rm -rf /opt/nrb"),
        "успешная установка ничего не откатывает"
    );
    assert!(
        fake.uploads
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.starts_with("/tmp/nrb-deploy-")),
        "бинарь заливается во временный файл"
    );
    assert!(
        fake.ran("--public-host '203.0.113.7' --port 443"),
        "ключи рождаются на сервере с нужным адресом и портом"
    );
}

#[tokio::test]
async fn existing_identity_is_kept_on_redeploy() {
    let mut rules = happy_rules(probe_text(
        "installed=nrb 0.0.0\nservice_active=yes\nport_busy=yes\nnode_port=443\n",
    ));
    rules.insert(0, ("[ -f /var/lib/nrb-node/node.toml ]", out(0, "")));
    let fake = Fake::new(rules);
    let report = run(&fake, &FakeVerifier(Ok(vec![])), &options())
        .await
        .unwrap();
    assert!(report.upgraded);
    assert!(
        !fake.ran("node init"),
        "существующие секреты узла не пересоздаются"
    );
}

#[tokio::test]
async fn unsuitable_servers_are_refused_before_any_change() {
    let cases = [
        ("kernel=FreeBSD\n", "Linux"),
        ("systemd=no\n", "systemd"),
        ("uid=1000\nsudo=no\n", "sudo"),
        ("arch=aarch64\n", "архитектур"),
        ("disk_mb=20\n", "мало места"),
        ("port_busy=yes\n", "занят"),
        (
            "node_port=8443\ninstalled=nrb 0.0.0\nservice_active=yes\nport_busy=yes\n",
            "порту 8443",
        ),
    ];
    for (probe_override, expected) in cases {
        // Последнее значение ключа побеждает: переопределяем поля в конце текста.
        let fake = Fake::new(happy_rules(probe_text(probe_override)));
        let error = run(&fake, &FakeVerifier(Ok(vec![])), &options())
            .await
            .unwrap_err();
        assert!(
            matches!(error, DeployError::Unsuitable(_)),
            "{probe_override}: {error}"
        );
        assert!(
            error
                .to_string()
                .to_lowercase()
                .contains(&expected.to_lowercase()),
            "{probe_override}: сообщение «{error}» не содержит «{expected}»"
        );
        assert_eq!(
            fake.log().len(),
            1,
            "до изменений дело дойти не должно (только probe): {:?}",
            fake.log()
        );
        assert!(fake.uploads.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn verification_failure_on_fresh_install_removes_everything_created() {
    let fake = Fake::new(happy_rules(probe_text("")));
    let error = run(
        &fake,
        &FakeVerifier(Err("хендшейк не прошёл".into())),
        &options(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, DeployError::Verify(_)));
    for needle in [
        "systemctl disable --now nrb-node",
        "rm -f /etc/systemd/system/nrb-node.service",
        "rm -rf /opt/nrb",
        "rm -rf /var/lib/nrb-node",
        "userdel nrb-node",
    ] {
        assert!(
            fake.ran(needle),
            "откат не выполнил «{needle}». Журнал:\n{}",
            fake.log().join("\n")
        );
    }
    assert!(
        fake.ran("node revoke"),
        "проверочное устройство отзывается и при сбое"
    );
}

#[tokio::test]
async fn verification_failure_on_upgrade_restores_previous_version_and_keeps_data() {
    let mut rules = happy_rules(probe_text(
        "installed=nrb 0.0.0\nservice_active=yes\nport_busy=yes\nnode_port=443\n",
    ));
    rules.insert(0, ("[ -f /var/lib/nrb-node/node.toml ]", out(0, "")));
    rules.insert(0, ("getent passwd", out(0, "")));
    let fake = Fake::new(rules);
    let error = run(
        &fake,
        &FakeVerifier(Err("нет выхода наружу".into())),
        &options(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, DeployError::Verify(_)));
    assert!(
        fake.ran("mv -f /opt/nrb/nrb.prev /opt/nrb/nrb"),
        "возвращаем прежний бинарь"
    );
    assert!(
        fake.ran("systemctl daemon-reload; systemctl restart nrb-node"),
        "и перезапускаем службу"
    );
    for forbidden in ["userdel", "rm -rf /var/lib/nrb-node", "rm -rf /opt/nrb"] {
        assert!(
            !fake.ran(forbidden),
            "апгрейд при откате не должен делать «{forbidden}»"
        );
    }
}

#[tokio::test]
async fn corrupted_upload_is_caught_before_replacing_the_binary() {
    let mut rules = happy_rules(probe_text(""));
    rules.insert(0, ("sha256sum", out(0, "deadbeef\n")));
    let fake = Fake::new(rules);
    let error = run(&fake, &FakeVerifier(Ok(vec![])), &options())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("контрольная сумма"), "{error}");
    assert!(
        !fake.ran("mv -f /opt/nrb/nrb.new"),
        "битый файл не должен заменить бинарь"
    );
}

#[tokio::test]
async fn binary_that_does_not_start_on_the_server_is_rejected() {
    let mut rules = happy_rules(probe_text(""));
    rules.insert(
        0,
        (
            ".new --version",
            ExecOutput {
                code: 127,
                stdout: String::new(),
                stderr: "GLIBC_2.39 not found".into(),
            },
        ),
    );
    let fake = Fake::new(rules);
    let error = run(&fake, &FakeVerifier(Ok(vec![])), &options())
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("musl") && text.contains("GLIBC"), "{text}");
    assert!(!fake.ran("mv -f /opt/nrb/nrb.new"));
    assert!(
        fake.ran("rm -f /opt/nrb/nrb.new"),
        "несовместимый файл удаляется"
    );
}

#[tokio::test]
async fn user_input_is_always_shell_quoted() {
    let fake = Fake::new(happy_rules(probe_text("")));
    let mut opts = options();
    opts.device_name = "x'; reboot; echo '".into();
    opts.public_host = "h$(id)".into();
    run(&fake, &FakeVerifier(Ok(vec![])), &opts).await.unwrap();
    let log = fake.log().join("\n");
    assert!(
        log.contains("'x'\\''; reboot; echo '\\'''"),
        "имя устройства не экранировано. Журнал:\n{log}"
    );
    assert!(log.contains("--public-host 'h$(id)'"));
    assert!(
        !log.contains("; reboot; echo '\n"),
        "инъекция не должна остаться неэкранированной"
    );
}

#[tokio::test]
async fn non_root_user_runs_everything_through_sudo() {
    let mut rules = happy_rules(probe_text("uid=1000\nsudo=yes\n"));
    rules.insert(0, ("sudo -n sh -c 'getent passwd", out(2, "")));
    let fake = Fake::new(rules);
    run(&fake, &FakeVerifier(Ok(vec![])), &options())
        .await
        .unwrap();
    let log = fake.log();
    assert!(
        log[1..]
            .iter()
            .all(|c| c.starts_with("sudo -n sh -c ") || c.starts_with("sudo -n ")),
        "команды после probe без sudo: {:?}",
        log.iter()
            .filter(|c| !c.starts_with("sudo"))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn dry_run_changes_nothing() {
    let fake = Fake::new(happy_rules(probe_text("")));
    let mut opts = options();
    opts.dry_run = true;
    let report = run(&fake, &FakeVerifier(Ok(vec![])), &opts).await.unwrap();
    assert!(report.invite.is_empty());
    assert_eq!(fake.log().len(), 1);
    assert!(fake.uploads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ufw_rules_are_opened_and_rolled_back_with_the_install() {
    let fake = Fake::new(happy_rules(probe_text("ufw=active\n")));
    let error = run(&fake, &FakeVerifier(Err("x".into())), &options())
        .await
        .unwrap_err();
    assert!(matches!(error, DeployError::Verify(_)));
    assert!(fake.ran("ufw allow 443/tcp && ufw allow 443/udp"));
    assert!(
        fake.ran("ufw delete allow 443/tcp"),
        "правило файрвола, открытое установкой, закрывается при откате"
    );
}

#[test]
fn probe_parsing() {
    let probe = parse_probe(&probe_text(
        "installed=nrb 0.0.1\nservice_active=yes\nnode_port=443\nufw=active\n",
    ));
    assert_eq!(probe.installed_version.as_deref(), Some("nrb 0.0.1"));
    assert!(probe.service_active && probe.ufw_active && probe.systemd && probe.bbr_available);
    assert_eq!(probe.node_port, Some(443));
    assert_eq!(probe.disk_mb, 9000);
    assert_eq!(parse_probe("мусор без равно").uid, 0);
}

#[tokio::test]
async fn injected_failure_triggers_rollback_like_a_real_one() {
    let fake = Fake::new(happy_rules(probe_text("")));
    let mut opts = options();
    opts.inject_verify_failure = true;
    let error = run(&fake, &FakeVerifier(Ok(vec![])), &opts)
        .await
        .unwrap_err();
    assert!(matches!(error, DeployError::Verify(_)));
    assert!(fake.ran("rm -rf /opt/nrb"));
}
