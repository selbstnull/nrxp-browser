//! Герметичный режим (I11): `--hermetic` и самопроверка `nrb hermetic-selftest`. Устройство и ограничения — `nrb-hermetic`.
//!
//! С форком `nrxp-servo` (N2) движок вообще не открывает сокетов: все TCP-соединения идут через `NetworkConnector` в NRXP-поток,
//! который живёт в рантайме туннеля (сеть хоста). Поэтому здесь остаётся только вход в пустое сетевое пространство имён —
//! релей и unix-сокет прокси больше не нужны.

use std::time::Duration;

use nrb_transport::Transport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Звать первым делом в `main`, пока процесс однопоточный: при `--hermetic`/`NRB_HERMETIC=1` (и для самопроверки)
/// входит в пространство имён пользователя. Не вышло — выход с ошибкой: молча деградировать в негерметичный режим нельзя.
pub fn early_init() {
    let selftest = std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "hermetic-selftest");
    if !(selftest || nrb_hermetic::requested_early()) {
        return;
    }
    if let Err(error) = nrb_hermetic::enter_user_namespace() {
        eprintln!("герметичный режим недоступен: {error}");
        std::process::exit(3);
    }
}

/// Входит в пустое сетевое пространство имён **этим потоком** (потоки, которые он создаст дальше, наследуют его) и проверяет,
/// что в нём остался только `lo`. Вызывать после создания туннеля (его потоки остаются в сети хоста) и перед созданием движка.
pub fn enter_empty_namespace() -> Result<(), String> {
    nrb_hermetic::enter_empty_network_namespace()
        .map_err(|e| format!("пустое сетевое пространство имён: {e}"))?;
    if !nrb_hermetic::is_empty_namespace().map_err(|e| e.to_string())? {
        return Err("после unshare в пространстве имён остались интерфейсы кроме lo".into());
    }
    Ok(())
}

fn report(ok: bool, text: &str, failed: &mut bool) {
    if ok {
        println!("ok   {text}");
    } else {
        println!("FAIL {text}");
        *failed = true;
    }
}

/// Самопроверка: после входа в герметичный режим прямые соединения (TCP хоста, интернет, UDP, DNS) невозможны, а поток к
/// внешнему хосту через туннель открывается — тем же способом, каким это делает движок (`NetworkConnector` → `Transport`).
pub fn selftest(args: &crate::tunnel::NodeArgs, host_port: Option<u16>) -> i32 {
    let mut tunnel = match crate::tunnel::start(args) {
        Ok(tunnel) => tunnel,
        Err(error) => {
            eprintln!("туннель: {error}");
            return 3;
        }
    };
    let host_ns = nrb_hermetic::census()
        .map(|c| c.namespaces())
        .unwrap_or_default();
    if let Err(error) = tunnel.isolate(true) {
        eprintln!("{error}");
        return 3;
    }
    let mut failed = false;

    report(
        nrb_hermetic::is_empty_namespace().unwrap_or(false),
        "в пространстве имён движка только lo",
        &mut failed,
    );
    let census = nrb_hermetic::census().unwrap();
    let ns_now = census
        .threads
        .iter()
        .find(|(_, name)| name == "nrb")
        .map(|(ns, _)| *ns)
        .or_else(|| census.threads.first().map(|(ns, _)| *ns))
        .unwrap_or(0);
    let current = std::fs::read_link("/proc/thread-self/ns/net")
        .ok()
        .and_then(|l| {
            l.to_string_lossy()
                .trim_start_matches("net:[")
                .trim_end_matches(']')
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(ns_now);
    let tunnel_threads: Vec<_> = census
        .threads
        .iter()
        .filter(|(_, name)| name.starts_with("nrb-tunnel"))
        .collect();
    report(
        !tunnel_threads.is_empty()
            && tunnel_threads
                .iter()
                .all(|(ns, _)| *ns != current && host_ns.contains(ns)),
        &format!(
            "все потоки туннеля ({}) остались в сети хоста",
            tunnel_threads.len()
        ),
        &mut failed,
    );
    let spawned_empty = std::thread::spawn(|| nrb_hermetic::is_empty_namespace().unwrap_or(false))
        .join()
        .unwrap_or(false);
    report(
        spawned_empty,
        "новый поток наследует пустое пространство имён",
        &mut failed,
    );

    // Прямые соединения обязаны не получиться.
    if let Some(port) = host_port {
        let direct = std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_secs(2),
        );
        report(
            direct.is_err(),
            &format!(
                "loopback хоста 127.0.0.1:{port} недоступен ({})",
                direct.err().map_or("?".into(), |e| e.to_string())
            ),
            &mut failed,
        );
    }
    let internet =
        std::net::TcpStream::connect_timeout(&([1, 1, 1, 1], 443).into(), Duration::from_secs(3));
    report(
        internet.is_err(),
        &format!(
            "прямое TCP-соединение в интернет невозможно ({})",
            internet.err().map_or("?".into(), |e| e.to_string())
        ),
        &mut failed,
    );
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|s| s.send_to(b"x", "8.8.8.8:53"));
    report(
        udp.is_err(),
        &format!(
            "прямой UDP в интернет невозможен ({})",
            udp.err().map_or("?".into(), |e| e.to_string())
        ),
        &mut failed,
    );
    let dns = std::net::ToSocketAddrs::to_socket_addrs(&("example.org", 80));
    report(
        dns.is_err(),
        "DNS-имена локально не резолвятся",
        &mut failed,
    );

    // Единственный путь наружу — поток NRXP: открываем его так же, как коннектор движка (из пустого ns, задача — в рантайме
    // туннеля), и читаем страницу.
    let via = fetch_via_transport(&tunnel, "example.org", 80);
    report(
        via.as_deref()
            .is_ok_and(|body| body.contains("Example Domain")),
        &format!(
            "через туннель страница открывается ({})",
            via.as_ref()
                .map_or_else(|e| e.clone(), |b| format!("{} байт", b.len()))
        ),
        &mut failed,
    );

    drop(tunnel);
    println!(
        "\n{}",
        if failed {
            "hermetic-selftest: есть падения"
        } else {
            "hermetic-selftest: всё прошло"
        }
    );
    i32::from(failed)
}

/// HTTP/1.0 GET по потоку, открытому через транспорт (как `NetworkConnector` движка).
fn fetch_via_transport(
    tunnel: &crate::tunnel::Tunnel,
    host: &str,
    port: u16,
) -> Result<String, String> {
    let transport = tunnel.transport();
    let handle = tunnel.runtime_handle();
    let host = host.to_owned();
    handle.block_on(async move {
        let stream = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::spawn({
                let (transport, host) = (transport.clone(), host.clone());
                async move { transport.open_tcp(&host, port).await }
            }),
        )
        .await
        .map_err(|_| "таймаут открытия потока".to_owned())?
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let mut stream = stream;
        stream
            .write_all(
                format!("GET / HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut body = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), stream.read_to_end(&mut body))
            .await
            .map_err(|_| "таймаут чтения".to_owned())?
            .map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    })
}
