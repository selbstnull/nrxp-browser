//! Полный цикл без бэкенда: узел с локальным реестром, спаренное устройство, чужие и отозванные устройства.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nrb_node::{DeviceStore, IngressHandle, Invite, NodeFile, run_ingress};
use nrb_transport::{NodeConfig, NrxpTransport, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Fixture {
    _dir: tempfile::TempDir,
    node: NodeFile,
    store: Arc<DeviceStore>,
    ingress: IngressHandle,
    echo_port: u16,
}

async fn echo() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = socket.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let node = NodeFile::init(dir.path(), "127.0.0.1", 0).unwrap();
    let store = Arc::new(DeviceStore::open(dir.path()).unwrap());
    let ingress = run_ingress(&node, store.clone()).await.unwrap();
    Fixture {
        _dir: dir,
        node,
        store,
        ingress,
        echo_port: echo().await,
    }
}

impl Fixture {
    fn invite(&self, token: &str) -> Invite {
        Invite {
            host: "127.0.0.1".into(),
            port: self.ingress.local_addr.port(),
            public_key: self.node.public_key().unwrap(),
            secret: self.node.secret.clone(),
            token: token.to_owned(),
            decoy_sni: self.node.decoy_host.clone(),
            name: "test".into(),
        }
    }

    fn config(&self, token: &str) -> NodeConfig {
        let mut config = self.invite(token).to_node_config();
        config.connect_timeout_secs = 5;
        config
    }
}

async fn echo_roundtrip(transport: &NrxpTransport, port: u16) -> std::io::Result<()> {
    let mut stream = transport.open_tcp("127.0.0.1", port).await?;
    stream.write_all(b"ping").await?;
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut buf))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    assert_eq!(&buf, b"ping");
    Ok(())
}

/// «Не пущен» = подключение не поднялось ИЛИ поток к цели не работает.
async fn assert_rejected(who: &str, config: &NodeConfig, echo_port: u16) {
    if let Ok(transport) = NrxpTransport::connect(config).await {
        assert!(
            echo_roundtrip(&transport, echo_port).await.is_err(),
            "{who} получил рабочий прокси!"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn paired_device_gets_a_working_tunnel() {
    let f = fixture().await;
    let (_, token) = f.store.add_device("телефон").unwrap();
    let transport = NrxpTransport::connect(&f.config(&token))
        .await
        .expect("спаренное устройство должно подключиться");
    echo_roundtrip(&transport, f.echo_port).await.unwrap();
    assert!(f.ingress.active_sessions() >= 1);

    // Приглашение переживает сериализацию в ссылку и обратно — и работает.
    let link = f.invite(&token).to_url();
    let from_link = Invite::parse(&link).unwrap().to_node_config();
    let second = NrxpTransport::connect(&from_link).await.unwrap();
    echo_roundtrip(&second, f.echo_port).await.unwrap();
    f.ingress.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_token_and_wrong_secret_are_rejected() {
    let f = fixture().await;
    let (_, good) = f.store.add_device("свой").unwrap();
    // Верные секрет и ключ узла, но токен не из реестра.
    assert_rejected(
        "устройство с чужим токеном",
        &f.config(&"00".repeat(32)),
        f.echo_port,
    )
    .await;
    // Верный токен, но чужой секрет узла.
    let mut wrong_secret = f.config(&good);
    wrong_secret.nrxp_secret = Some("11".repeat(32));
    assert_rejected("клиент с чужим секретом", &wrong_secret, f.echo_port).await;
    // Анонимный клиент без учётных данных вообще.
    let mut anonymous = NodeConfig::anonymous(format!("127.0.0.1:{}", f.ingress.local_addr.port()));
    anonymous.connect_timeout_secs = 5;
    assert_rejected("анонимный клиент", &anonymous, f.echo_port).await;
    // И свой по-прежнему работает.
    let transport = NrxpTransport::connect(&f.config(&good)).await.unwrap();
    echo_roundtrip(&transport, f.echo_port).await.unwrap();
    f.ingress.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_device_cuts_its_live_session() {
    let f = fixture().await;
    let (_, token) = f.store.add_device("потеряный").unwrap();
    let transport = NrxpTransport::connect(&f.config(&token)).await.unwrap();
    let mut live = transport.open_tcp("127.0.0.1", f.echo_port).await.unwrap();
    live.write_all(b"x").await.unwrap();
    let mut one = [0u8; 1];
    live.read_exact(&mut one).await.unwrap();

    f.store.revoke("потеряный").unwrap();

    // Живая сессия закрывается (проверка реестра ≥1 с + обход отозванных каждые 2 с), новая — не создаётся.
    let started = Instant::now();
    let mut sink = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(20), live.read_to_end(&mut sink)).await;
    assert!(
        ended.is_ok(),
        "живой поток отозванного устройства не закрылся за 20 с"
    );
    eprintln!("поток закрыт через {:?} после отзыва", started.elapsed());
    assert_rejected("отозванное устройство", &f.config(&token), f.echo_port).await;
    f.ingress.shutdown().await;
}
