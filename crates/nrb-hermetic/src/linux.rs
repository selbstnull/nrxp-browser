use std::io;
use std::path::{Path, PathBuf};

fn last_os_error(context: &str) -> io::Error {
    let error = io::Error::last_os_error();
    io::Error::new(error.kind(), format!("{context}: {error}"))
}

fn threads_in_process() -> io::Result<usize> {
    Ok(std::fs::read_dir("/proc/self/task")?.count())
}

/// Новое пространство имён пользователя; вызывать первым делом, пока поток один (требование ядра).
pub fn enter_user_namespace() -> io::Result<()> {
    let threads = threads_in_process()?;
    if threads != 1 {
        return Err(io::Error::other(format!(
            "пространство имён пользователя создаётся только в однопоточном процессе (потоков: {threads})"
        )));
    }
    // SAFETY: чистые системные вызовы без указателей на память Rust.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    if unsafe { libc::unshare(libc::CLONE_NEWUSER) } != 0 {
        return Err(last_os_error(
            "unshare(CLONE_NEWUSER) — пользовательские пространства имён запрещены (sysctl user.max_user_namespaces / AppArmor)",
        ));
    }
    // Порядок важен: setgroups=deny, затем gid_map и uid_map.
    std::fs::write("/proc/self/setgroups", "deny")?;
    std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1"))?;
    std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1"))?;
    Ok(())
}

/// Пустое сетевое пространство имён для **вызывающего потока** (и всех потоков, которые он создаст дальше); поднимает `lo`.
pub fn enter_empty_network_namespace() -> io::Result<()> {
    // SAFETY: см. выше.
    if unsafe { libc::unshare(libc::CLONE_NEWNET) } != 0 {
        return Err(last_os_error(
            "unshare(CLONE_NEWNET) — нет прав на создание сетевого пространства имён (вызван ли enter_user_namespace?)",
        ));
    }
    bring_loopback_up()
}

fn bring_loopback_up() -> io::Result<()> {
    // SAFETY: структура ifreq обнуляется и заполняется до вызова ioctl; сокет закрывается.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if fd < 0 {
            return Err(last_os_error("socket"));
        }
        let mut request: libc::ifreq = std::mem::zeroed();
        for (slot, byte) in request.ifr_name.iter_mut().zip(b"lo\0") {
            *slot = *byte as libc::c_char;
        }
        let result = if libc::ioctl(fd, libc::SIOCGIFFLAGS as _, &mut request) != 0 {
            Err(last_os_error("SIOCGIFFLAGS"))
        } else {
            request.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            if libc::ioctl(fd, libc::SIOCSIFFLAGS as _, &request) != 0 {
                Err(last_os_error("SIOCSIFFLAGS (lo up)"))
            } else {
                Ok(())
            }
        };
        libc::close(fd);
        result
    }
}

/// Интерфейсы текущего пространства имён (из `/proc/self/net/dev`; `/proc/self` — процесс, а `net` берётся для потока
/// через `/proc/thread-self`).
pub fn interfaces() -> io::Result<Vec<String>> {
    let text = std::fs::read_to_string("/proc/thread-self/net/dev")?;
    Ok(text
        .lines()
        .skip(2)
        .filter_map(|l| l.split(':').next())
        .map(|n| n.trim().to_owned())
        .collect())
}

/// Герметично ли текущее пространство имён потока: единственный интерфейс — `lo`.
pub fn is_empty_namespace() -> io::Result<bool> {
    Ok(interfaces()? == ["lo"])
}

/// Релей: TCP-порт на loopback пустого ns → unix-сокет прокси. Поток релея обязан создаваться уже внутри пустого ns.
pub struct Relay {
    pub port: u16,
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start_relay(proxy_socket: &Path) -> io::Result<Relay> {
    if !is_empty_namespace()? {
        return Err(io::Error::other(
            "релей запускается только внутри пустого сетевого пространства имён",
        ));
    }
    let path: PathBuf = proxy_socket.to_owned();
    let (port_tx, port_rx) = std::sync::mpsc::channel::<io::Result<u16>>();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("nrb-relay".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = port_tx.send(Err(error));
                    return;
                }
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ = port_tx.send(Err(error));
                        return;
                    }
                };
                let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                let _ = port_tx.send(Ok(port));
                let stop = async {
                    loop {
                        if stop_rx.try_recv().is_ok() {
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                };
                tokio::pin!(stop);
                loop {
                    tokio::select! {
                        _ = &mut stop => return,
                        accepted = listener.accept() => {
                            let Ok((mut client, _)) = accepted else { continue };
                            let path = path.clone();
                            tokio::spawn(async move {
                                let Ok(mut upstream) = tokio::net::UnixStream::connect(&path).await else { return };
                                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                            });
                        }
                    }
                }
            });
        })?;
    let port = port_rx
        .recv()
        .map_err(|_| io::Error::other("поток релея завершился при запуске"))??;
    Ok(Relay {
        port,
        stop: stop_tx,
        thread: Some(thread),
    })
}
