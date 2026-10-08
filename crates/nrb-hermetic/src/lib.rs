//! Герметичный режим (I11, Linux). Гарантия «по построению»: потоки движка живут в **пустом сетевом пространстве имён**
//! (есть только `lo`, маршрутов нет), поэтому любая попытка прямого соединения — страницей, WebRTC, новой версией Servo с
//! забытым сокетом — завершается ошибкой, а не утечкой. Наружу ведёт единственный путь:
//! `движок → релей на loopback пустого ns → unix-сокет → локальный прокси → туннель NRXP`.
//!
//! Как это устроено (без root):
//! 1. [`enter_user_namespace`] — самое первое действие процесса, пока он однопоточный: новое пространство имён пользователя
//!    (uid/gid отображаются сами на себя). Внутри процесс получает права для создания сетевого пространства имён;
//! 2. туннель, прокси и всё, что должно ходить в сеть (рантаймы tokio), создаются **до** шага 3 и остаются в сети хоста;
//! 3. [`enter_empty_network_namespace`] зовётся из потока, который затем создаёт движок: `unshare(CLONE_NEWNET)` действует на
//!    вызывающий поток, а все потоки, порождённые им позже (в том числе потоки Servo), наследуют пустое пространство;
//! 4. [`start_relay`] — поток в пустом ns: TCP `127.0.0.1:порт` → unix-сокет прокси. Unix-сокеты по пути видны из любого ns.
//!
//! Ограничения: нужен разрешённый пользовательский ns (на Ubuntu 24.04 с `apparmor_restrict_unprivileged_userns=1`
//! неопознанным бинарям он может быть закрыт — тогда режим отказывает, а не молча деградирует); абстрактные unix-сокеты
//! (`@…`) в пустом ns недоступны — X11 по пути `/tmp/.X11-unix/…` работает; поток, созданный до шага 3, остаётся в сети хоста —
//! [`census`] показывает, какие потоки где.

use std::io;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(target_os = "linux"))]
mod other {
    use std::io;
    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "герметичный режим реализован только для Linux",
        ))
    }
    pub fn enter_user_namespace() -> io::Result<()> {
        unsupported()
    }
    pub fn enter_empty_network_namespace() -> io::Result<()> {
        unsupported()
    }
}
#[cfg(not(target_os = "linux"))]
pub use other::*;

/// Просьба включить герметичный режим: флаг `--hermetic` в аргументах или `NRB_HERMETIC=1`. Разбор намеренно примитивный:
/// он нужен до создания потоков и до `clap`.
pub fn requested_early() -> bool {
    std::env::args_os().any(|a| a == "--hermetic")
        || std::env::var_os("NRB_HERMETIC").is_some_and(|v| v == "1")
}

/// Сводка по потокам процесса: сколько в каком сетевом пространстве имён (идентификатор — inode `/proc/…/ns/net`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Census {
    /// `(inode ns, имя потока)` для каждого потока.
    pub threads: Vec<(u64, String)>,
}

impl Census {
    pub fn namespaces(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.threads.iter().map(|(ns, _)| *ns).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub fn in_namespace(&self, ns: u64) -> Vec<&str> {
        self.threads
            .iter()
            .filter(|(id, _)| *id == ns)
            .map(|(_, name)| name.as_str())
            .collect()
    }
}

/// Перечисляет потоки текущего процесса и их сетевые пространства имён.
pub fn census() -> io::Result<Census> {
    let mut threads = Vec::new();
    for entry in std::fs::read_dir("/proc/self/task")? {
        let path = entry?.path();
        let ns = match std::fs::read_link(path.join("ns/net")) {
            Ok(link) => link
                .to_string_lossy()
                .trim_start_matches("net:[")
                .trim_end_matches(']')
                .parse::<u64>()
                .unwrap_or(0),
            Err(_) => continue, // поток успел завершиться
        };
        let name = std::fs::read_to_string(path.join("comm"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        threads.push((ns, name));
    }
    Ok(Census { threads })
}
