//! Reverse-egress (I9): домашний узел дозванивается до ingress и становится выходом для его клиентов.
//! Цель теста — эхо-сервер на loopback этого же хоста, поэтому «вышло через egress» отличаем от «вышло с узла»
//! по режиму `require`: без живого egress поток обязан не открыться (иначе трафик ушёл бы с самого узла).

use std::sync::Arc;
use std::time::{Duration, Instant};

use nrb_node::{
    DeviceStore, Egress, EgressOptions, IngressHandle, Invite, NodeFile, Role, run_ingress,
};
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

async fn fixture(exit: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut node = NodeFile::init(dir.path(), "127.0.0.1", 0).unwrap();
    node.exit = exit.to_owned();
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
    fn config(&self, token: &str) -> NodeConfig {
        let mut config = Invite {
            host: "127.0.0.1".into(),
            port: self.ingress.local_addr.port(),
            public_key: self.node.public_key().unwrap(),
            secret: self.node.secret.clone(),
            token: token.to_owned(),
            decoy_sni: self.node.decoy_host.clone(),
            name: "test".into(),
        }
        .to_node_config();
        config.connect_timeout_secs = 5;
        config
    }

    async fn start_egress(&self, allow_lan: bool) -> Egress {
        let (_, token) = self
            .store
            .add_device_with_role("дом", Role::Egress)
            .unwrap();
        let egress = Egress::start(
            &self.config(&token),
            &EgressOptions {
                label: "дом".into(),
                allow_private: allow_lan,
            },
            Duration::from_secs(5),
        )
        .await
        .expect("выход должен подключиться");
        self.wait_live(1).await;
        egress
    }

    async fn wait_live(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while self.ingress.live_egress().len() != count {
            assert!(
                Instant::now() < deadline,
                "ждали {count} живых выходов, сейчас {:?}",
                self.ingress.live_egress()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn client(&self) -> NrxpTransport {
        let name = format!("клиент-{}", rand_suffix());
        let (_, token) = self.store.add_device(&name).unwrap();
        NrxpTransport::connect(&self.config(&token)).await.unwrap()
    }
}

fn rand_suffix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
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

#[tokio::test(flavor = "multi_thread")]
async fn clients_exit_through_the_home_egress_and_never_without_it() {
    let f = fixture("require").await;
    let client = f.client().await;

    // Выхода нет: режим require — отказ, а не выход с узла (эхо на loopback узла достижимо, но трафик не должен туда идти).
    assert!(
        echo_roundtrip(&client, f.echo_port).await.is_err(),
        "без egress трафик не должен выходить с самого узла"
    );

    let egress = f.start_egress(true).await;
    echo_roundtrip(&client, f.echo_port)
        .await
        .expect("через домашний egress поток открывается");

    // Выход пропал — клиенты не выходят «заодно» с узла.
    drop(egress);
    let deadline = Instant::now() + Duration::from_secs(8);
    while echo_roundtrip(&client, f.echo_port).await.is_ok() {
        assert!(
            Instant::now() < deadline,
            "после отключения egress поток всё ещё открывается"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    f.ingress.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn home_lan_is_closed_by_default() {
    let f = fixture("require").await;
    let _egress = f.start_egress(false).await;
    let client = f.client().await;
    assert!(
        echo_roundtrip(&client, f.echo_port).await.is_err(),
        "loopback домашнего устройства не должен быть достижим через выход"
    );
    f.ingress.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prefer_falls_back_to_the_node_when_no_egress() {
    let f = fixture("prefer").await;
    let client = f.client().await;
    echo_roundtrip(&client, f.echo_port).await.unwrap();
    f.ingress.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn roles_do_not_mix() {
    let f = fixture("require").await;
    // Клиентский токен выходом стать не может.
    let (_, client_token) = f.store.add_device("телефон").unwrap();
    let rejected = Egress::start(
        &f.config(&client_token),
        &EgressOptions::default(),
        Duration::from_secs(3),
    )
    .await;
    assert!(rejected.is_err(), "клиентский токен не даёт роль выхода");
    assert!(f.ingress.live_egress().is_empty());

    // А токен выхода не работает как клиентский.
    let (_, egress_token) = f.store.add_device_with_role("дом", Role::Egress).unwrap();
    let _egress = Egress::start(
        &f.config(&egress_token),
        &EgressOptions {
            label: "дом".into(),
            allow_private: true,
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    f.wait_live(1).await;
    if let Ok(transport) = NrxpTransport::connect(&f.config(&egress_token)).await {
        assert!(echo_roundtrip(&transport, f.echo_port).await.is_err());
    }
    f.ingress.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_the_egress_removes_it_from_routes() {
    let f = fixture("require").await;
    let _egress = f.start_egress(true).await;
    let client = f.client().await;
    echo_roundtrip(&client, f.echo_port).await.unwrap();

    f.store.revoke("дом").unwrap();
    f.wait_live(0).await;
    // Остальные клиенты и сам узел живы: клиент по-прежнему подключён, но выхода у него теперь нет.
    assert!(echo_roundtrip(&client, f.echo_port).await.is_err());
    f.ingress.shutdown().await;
}
