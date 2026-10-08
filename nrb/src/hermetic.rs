//! Герметичный режим (I11): `--hermetic` и самопроверка `nrb hermetic-selftest`. Устройство и ограничения — `nrb-hermetic`.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

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

/// Каталог для unix-сокета прокси: `$XDG_RUNTIME_DIR/nrb-<pid>` (иначе `/tmp/nrb-<uid>-<pid>`), права 0700.
pub fn socket_dir() -> std::io::Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("nrb-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Результат изоляции: URL прокси для движка (через релей) и релей. Каталог сокета убирается при выходе.
pub struct Isolated {
    pub url: String,
    pub _relay: nrb_hermetic::Relay,
    pub dir: PathBuf,
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Входит в пустое сетевое пространство имён **этим потоком** и поднимает релей. Вызывать после создания туннеля
/// (его потоки остаются в сети хоста) и перед созданием движка (его потоки наследуют пустое пространство).
pub fn isolate(
    socket: &std::path::Path,
    credentials: (&str, &str),
) -> Result<(nrb_hermetic::Relay, String), String> {
    nrb_hermetic::enter_empty_network_namespace()
        .map_err(|e| format!("пустое сетевое пространство имён: {e}"))?;
    if !nrb_hermetic::is_empty_namespace().map_err(|e| e.to_string())? {
        return Err("после unshare в пространстве имён остались интерфейсы кроме lo".into());
    }
    let relay = nrb_hermetic::start_relay(socket).map_err(|e| format!("релей: {e}"))?;
    let url = format!(
        "http://{}:{}@127.0.0.1:{}",
        credentials.0, credentials.1, relay.port
    );
    Ok((relay, url))
}

fn report(ok: bool, text: &str, failed: &mut bool) {
    if ok {
        println!("ok   {text}");
    } else {
        println!("FAIL {text}");
        *failed = true;
    }
}

/// Самопроверка: после входа в герметичный режим прямые соединения (TCP хоста, интернет, UDP) невозможны, а через релей
/// страница открывается; потоки туннеля остались в сети хоста.
pub fn selftest(args: &crate::tunnel::NodeArgs, host_port: Option<u16>) -> i32 {
    let dir = match socket_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("каталог сокета: {error}");
            return 3;
        }
    };
    let socket = dir.join("proxy.sock");
    let tunnel = match crate::tunnel::start_unix(args, &socket) {
        Ok(tunnel) => tunnel,
        Err(error) => {
            eprintln!("туннель: {error}");
            let _ = std::fs::remove_dir_all(&dir);
            return 3;
        }
    };
    let before = nrb_hermetic::census().unwrap_or(nrb_hermetic::Census { threads: vec![] });
    let host_ns = before.namespaces();
    let creds = tunnel.credentials();
    let (relay, proxy_url) = match isolate(&socket, creds) {
        Ok(done) => done,
        Err(error) => {
            eprintln!("{error}");
            let _ = std::fs::remove_dir_all(&dir);
            return 3;
        }
    };
    let mut failed = false;
    let ns_now = nrb_hermetic::census()
        .ok()
        .and_then(|c| {
            c.threads
                .iter()
                .find(|(_, name)| name == "nrb-relay")
                .map(|(ns, _)| *ns)
        })
        .unwrap_or(0);

    report(
        nrb_hermetic::is_empty_namespace().unwrap_or(false),
        "в пространстве имён движка только lo",
        &mut failed,
    );
    let census = nrb_hermetic::census().unwrap();
    let tunnel_threads: Vec<_> = census
        .threads
        .iter()
        .filter(|(_, name)| name.starts_with("nrb-tunnel"))
        .collect();
    report(
        !tunnel_threads.is_empty()
            && tunnel_threads
                .iter()
                .all(|(ns, _)| *ns != ns_now && host_ns.contains(ns)),
        &format!(
            "все потоки туннеля ({}) остались в сети хоста",
            tunnel_threads.len()
        ),
        &mut failed,
    );
    report(
        ns_now != 0 && !host_ns.contains(&ns_now),
        "релей работает в пустом пространстве имён",
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

    // Единственный путь наружу — релей → прокси → туннель.
    let via = via_relay(relay.port, &proxy_url, "example.org", 80);
    report(
        via.as_deref()
            .is_ok_and(|body| body.contains("Example Domain")),
        &format!(
            "через релей и туннель страница открывается ({})",
            via.as_ref()
                .map_or_else(|e| e.clone(), |b| format!("{} байт", b.len()))
        ),
        &mut failed,
    );
    let wrong = via_relay(
        relay.port,
        "http://nrb-x:wrong@127.0.0.1",
        "example.org",
        80,
    );
    report(
        wrong.as_ref().err().is_some_and(|e| e.contains("407")),
        "без верного пароля прокси релей ничего не даёт (407)",
        &mut failed,
    );

    drop(relay);
    drop(tunnel);
    let _ = std::fs::remove_dir_all(&dir);
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

/// HTTP/1.0 GET через CONNECT на релей; ошибка — строка с ответом прокси.
fn via_relay(port: u16, proxy_url: &str, host: &str, target_port: u16) -> Result<String, String> {
    let creds = proxy_url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('@').next())
        .unwrap_or("");
    let auth = base64(creds.as_bytes());
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    write!(
        stream,
        "CONNECT {host}:{target_port} HTTP/1.1\r\nHost: {host}:{target_port}\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).map_err(|e| e.to_string())? == 0 {
            break;
        }
        head.push(byte[0]);
        if head.len() > 8192 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    if !head.starts_with("HTTP/1.1 200") {
        return Err(head.lines().next().unwrap_or("нет ответа").to_owned());
    }
    write!(
        stream,
        "GET / HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut body = Vec::new();
    stream.read_to_end(&mut body).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
