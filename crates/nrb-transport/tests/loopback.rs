//! Интеграция: локальный `netrunner-server` (анонимная схема, loopback) из соседнего репозитория +
//! эхо-цель. Бинарь ищется в `../netrunner-proxy/target/{release,debug}` или в `$NRB_SERVER_BIN`;
//! нет бинаря — тесты пропускаются с пояснением (CI без соседнего репо не краснеет).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nrb_transport::{NodeConfig, NrxpTransport, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn server_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NRB_SERVER_BIN") {
        return Some(PathBuf::from(path));
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../netrunner-proxy/target");
    ["release", "debug"]
        .iter()
        .map(|profile| root.join(profile).join("netrunner-server"))
        .find(|p| p.exists())
}

struct Node {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start_node() -> Option<Node> {
    start_node_with_env(&[]).await
}

async fn start_node_with_env(envs: &[(&str, String)]) -> Option<Node> {
    start_node_at(None, envs).await
}

async fn start_node_at(fixed_port: Option<u16>, envs: &[(&str, String)]) -> Option<Node> {
    let Some(binary) = server_binary() else {
        eprintln!(
            "ПРОПУСК: нет netrunner-server (соберите `cargo build -p netrunner-server --release` в netrunner-proxy или задайте NRB_SERVER_BIN)"
        );
        return None;
    };
    let port = fixed_port.unwrap_or_else(|| {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    });
    // NRB_TEST_SERVER_LOG=файл — писать лог узла (JSON в stdout) для разбора падений.
    let log = std::env::var_os("NRB_TEST_SERVER_LOG").and_then(|p| std::fs::File::create(p).ok());
    let child = Command::new(binary)
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .envs(envs.iter().map(|(k, v)| (*k, v.as_str())))
        .stdout(log.map_or_else(Stdio::null, Stdio::from))
        .stderr(Stdio::null())
        .spawn()
        .expect("запуск netrunner-server");
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let node = Node { child, addr };
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_ok() {
            return Some(node);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("netrunner-server не начал слушать {addr}");
}

async fn transport(node: &Node) -> Arc<NrxpTransport> {
    let config = NodeConfig::anonymous(node.addr.to_string());
    Arc::new(
        NrxpTransport::connect(&config)
            .await
            .expect("подключение к узлу"),
    )
}

/// Эхо-сервер: возвращает всё, что получил.
async fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut reader, mut writer) = socket.split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
        }
    });
    addr
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| {
            (i as u8)
                .wrapping_mul(31)
                .wrapping_add(seed)
                .wrapping_add((i >> 8) as u8)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn echo_roundtrip() {
    let Some(node) = start_node().await else {
        return;
    };
    let transport = transport(&node).await;
    let echo = echo_server().await;

    let mut stream = transport.open_tcp("127.0.0.1", echo.port()).await.unwrap();
    stream.write_all(b"hello through nrxp").await.unwrap();
    let mut buf = [0u8; 18];
    tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
        .await
        .expect("таймаут чтения")
        .unwrap();
    assert_eq!(&buf, b"hello through nrxp");
}

#[tokio::test(flavor = "multi_thread")]
async fn large_transfer_keeps_integrity() {
    let Some(node) = start_node().await else {
        return;
    };
    let transport = transport(&node).await;
    let echo = echo_server().await;

    const TOTAL: usize = 20 * 1024 * 1024;
    let data = Arc::new(pattern(TOTAL, 7));
    let stream = transport.open_tcp("127.0.0.1", echo.port()).await.unwrap();
    let (mut reader, mut writer) = tokio::io::split(stream);

    let sent = data.clone();
    let write_task = tokio::spawn(async move {
        writer.write_all(&sent).await.unwrap();
        writer.flush().await.unwrap();
        writer
    });
    let mut received = vec![0u8; TOTAL];
    tokio::time::timeout(Duration::from_secs(120), reader.read_exact(&mut received))
        .await
        .expect("перекачка 20 МБ зависла")
        .unwrap();
    assert!(received == *data, "данные исказились при перекачке");
    drop(write_task.await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_close_gives_eof_after_all_data() {
    let Some(node) = start_node().await else {
        return;
    };
    let transport = transport(&node).await;

    // Цель шлёт 100 000 байт и закрывает соединение: хвост не должен потеряться, затем — EOF.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.write_all(&pattern(100_000, 3)).await.unwrap();
        socket.shutdown().await.unwrap();
    });

    let mut stream = transport.open_tcp("127.0.0.1", port).await.unwrap();
    let mut all = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut all))
        .await
        .expect("EOF не наступил")
        .unwrap();
    assert_eq!(all.len(), 100_000);
    assert_eq!(all, pattern(100_000, 3));
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_target_ends_the_stream() {
    let Some(node) = start_node().await else {
        return;
    };
    let transport = transport(&node).await;
    let dead_port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let mut stream = transport.open_tcp("127.0.0.1", dead_port).await.unwrap();
    let mut buf = Vec::new();
    let outcome = tokio::time::timeout(Duration::from_secs(15), stream.read_to_end(&mut buf))
        .await
        .expect("поток к закрытому порту не завершился");
    assert!(buf.is_empty(), "данных быть не должно");
    let _ = outcome; // EOF или ошибка — оба допустимы, главное, что не зависли
}

#[tokio::test(flavor = "multi_thread")]
async fn many_parallel_streams() {
    let Some(node) = start_node().await else {
        return;
    };
    let transport = transport(&node).await;
    let echo = echo_server().await;

    let streams: u8 = std::env::var("NRB_TEST_STREAMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(32);
    let size: usize = std::env::var("NRB_TEST_SIZE_KB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256)
        * 1024;
    let progress: Arc<Vec<std::sync::atomic::AtomicUsize>> =
        Arc::new((0..streams).map(|_| Default::default()).collect());

    let tasks: Vec<_> = (0..streams)
        .map(|n| {
            let transport = transport.clone();
            let progress = progress.clone();
            tokio::spawn(async move {
                let data = pattern(size, n);
                let stream = transport.open_tcp("127.0.0.1", echo.port()).await.unwrap();
                let (mut reader, mut writer) = tokio::io::split(stream);
                let to_send = data.clone();
                tokio::spawn(async move { writer.write_all(&to_send).await.unwrap() });
                let mut got = Vec::with_capacity(size);
                let mut buf = vec![0u8; 32 * 1024];
                let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                while got.len() < size {
                    match tokio::time::timeout_at(deadline, reader.read(&mut buf)).await {
                        Ok(Ok(0)) => panic!("поток {n}: EOF на {} из {size}", got.len()),
                        Ok(Ok(k)) => {
                            got.extend_from_slice(&buf[..k]);
                            progress[n as usize]
                                .store(got.len(), std::sync::atomic::Ordering::Relaxed);
                        }
                        Ok(Err(e)) => panic!("поток {n}: {e}"),
                        Err(_) => panic!(
                            "поток {n} завис на {} из {size} байт; прогресс всех: {:?}",
                            got.len(),
                            progress
                                .iter()
                                .map(|p| p.load(std::sync::atomic::Ordering::Relaxed) / 1024)
                                .collect::<Vec<_>>()
                        ),
                    }
                }
                assert!(got == data, "поток {n}: данные исказились");
            })
        })
        .collect();
    let mut failed = 0;
    for task in tasks {
        if task.await.is_err() {
            failed += 1;
        }
    }
    assert_eq!(failed, 0, "{failed} потоков из {streams} не завершились");
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_reader_does_not_kill_the_stream() {
    let Some(node) = start_node().await else {
        return;
    };
    let transport = transport(&node).await;

    // Цель быстро шлёт 24 МБ, клиент читает медленно: без кредитов поток убивался на лимите бэклога.
    const TOTAL: usize = 24 * 1024 * 1024;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let chunk = pattern(64 * 1024, 9);
        let mut sent = 0;
        while sent < TOTAL {
            if socket.write_all(&chunk).await.is_err() {
                return;
            }
            sent += chunk.len();
        }
        let _ = socket.shutdown().await;
    });

    let mut stream = transport.open_tcp("127.0.0.1", port).await.unwrap();
    let mut total = 0usize;
    let mut buf = vec![0u8; 64 * 1024];
    let started = std::time::Instant::now();
    loop {
        let n = tokio::time::timeout(Duration::from_secs(30), stream.read(&mut buf))
            .await
            .expect("чтение зависло")
            .unwrap();
        if n == 0 {
            break;
        }
        total += n;
        if total % (4 * 1024 * 1024) < n {
            tokio::time::sleep(Duration::from_millis(150)).await; // периодически «залипаем»
        }
    }
    assert_eq!(
        total,
        TOTAL,
        "потеряны данные при медленном чтении (за {:?})",
        started.elapsed()
    );
}

fn random_hex32() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(&mut bytes)
        .unwrap();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// S4: узел с секретом + приватным ключом + `PROXY_NRXP_STRICT=true` и БЕЗ бэкенда (`--require-auth` не задан).
/// Клиент с верными учётными данными работает; анонимный и с чужим секретом — нет (не получают прокси).
#[tokio::test(flavor = "multi_thread")]
async fn strict_node_admits_only_clients_with_credentials() {
    let secret = random_hex32();
    let private = random_hex32();
    let public = netrunner_core::LocalIdentity::from_hex(&secret, &private, true)
        .expect("ключи узла")
        .public_key_hex();
    let envs = [
        ("PROXY_NRXP_SECRET", secret.clone()),
        ("PROXY_NRXP_PRIVATE_KEY", private),
        ("PROXY_NRXP_STRICT", "true".to_owned()),
    ];
    let Some(node) = start_node_with_env(&envs).await else {
        return;
    };
    let echo = echo_server().await;

    let mut good = NodeConfig::anonymous(node.addr.to_string());
    good.nrxp_secret = Some(secret.clone());
    good.nrxp_public_key = Some(public.clone());
    let transport = NrxpTransport::connect(&good)
        .await
        .expect("клиент с верными учётными данными должен подключиться");
    let mut stream = transport.open_tcp("127.0.0.1", echo.port()).await.unwrap();
    stream.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
        .await
        .expect("таймаут")
        .unwrap();
    assert_eq!(&buf, b"ping");

    // Анонимный клиент: узел не должен пускать его в прокси.
    let mut anonymous = NodeConfig::anonymous(node.addr.to_string());
    anonymous.connect_timeout_secs = 4;
    assert_rejected("анонимный", &anonymous, echo.port()).await;

    // Верный публичный ключ, но чужой секрет.
    let mut wrong_secret = good.clone();
    wrong_secret.nrxp_secret = Some(random_hex32());
    wrong_secret.connect_timeout_secs = 4;
    assert_rejected("с чужим секретом", &wrong_secret, echo.port()).await;
}

/// «Не пущен» = подключение не поднялось ИЛИ поток к цели не отдаёт данных (decoy вместо прокси).
async fn assert_rejected(who: &str, config: &NodeConfig, echo_port: u16) {
    match NrxpTransport::connect(config).await {
        Err(_) => {}
        Ok(transport) => {
            let outcome = async {
                let mut stream = transport.open_tcp("127.0.0.1", echo_port).await?;
                stream.write_all(b"ping").await?;
                let mut buf = [0u8; 4];
                tokio::time::timeout(Duration::from_secs(4), stream.read_exact(&mut buf))
                    .await
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
                Ok::<_, std::io::Error>(buf)
            }
            .await;
            assert!(
                outcome.is_err(),
                "{who} клиент получил рабочий прокси на strict-узле без бэкенда!"
            );
        }
    }
}

async fn echo_once(transport: &NrxpTransport, port: u16) -> std::io::Result<()> {
    let mut stream = transport.open_tcp("127.0.0.1", port).await?;
    stream.write_all(b"ping").await?;
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut buf))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    assert_eq!(&buf, b"ping");
    Ok(())
}

/// Узел перезапускается: ядро само переподключает ноги (backoff), старые потоки умирают, новые снова работают
/// на той же сессии транспорта — браузеру не нужно пересоздавать `NrxpTransport`.
#[tokio::test(flavor = "multi_thread")]
async fn survives_node_restart() {
    let Some(node) = start_node().await else {
        return;
    };
    let port = node.addr.port();
    let transport = transport(&node).await;
    let echo = echo_server().await;
    echo_once(&transport, echo.port())
        .await
        .expect("до перезапуска узла эхо должно работать");

    // Поток, открытый до падения узла: когда пропадают все ноги, транспорт закрывает открытые потоки (EOF), а не оставляет висеть.
    let mut old_stream = transport.open_tcp("127.0.0.1", echo.port()).await.unwrap();
    old_stream.write_all(b"x").await.unwrap();
    drop(node); // kill

    let mut sink = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(30), old_stream.read_to_end(&mut sink))
        .await
        .expect("поток не закрылся после потери всех ног");
    // Пока туннель лежит, новые потоки отклоняются сразу (kill-switch), а не висят.
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        transport.open_tcp("127.0.0.1", echo.port()),
    )
    .await
    .expect("open_tcp завис при мёртвом туннеле");
    assert!(
        refused.is_err(),
        "при мёртвом туннеле open_tcp обязан вернуть ошибку"
    );

    let _node2 = start_node_at(Some(port), &[]).await.unwrap();
    let started = std::time::Instant::now();
    let mut recovered_after = None;
    while started.elapsed() < Duration::from_secs(90) {
        if echo_once(&transport, echo.port()).await.is_ok() {
            recovered_after = Some(started.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let recovered_after =
        recovered_after.expect("транспорт не восстановился за 90 с после перезапуска узла");
    eprintln!("восстановление после перезапуска узла: {recovered_after:?}");
}
