//! `nrb snap` — headless-снимок страницы (I1).

use std::path::PathBuf;
use std::time::Duration;

use nrb_engine::Size;
use url::Url;

#[cfg_attr(not(feature = "servo"), allow(dead_code))]
pub struct Args {
    pub url: Url,
    pub output: PathBuf,
    pub size: Size,
    pub timeout: Duration,
    pub settle: Duration,
    pub proxy: Option<String>,
    /// Печатать машиночитаемый отчёт (одна строка JSON) вместо человекочитаемой строки.
    pub json: bool,
    pub node: crate::tunnel::NodeArgs,
    pub profile: crate::profile::ProfileArgs,
}

/// Отчёт `nrb snap --json` (читает `nrb corpus`).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SnapReport {
    pub width: u32,
    pub height: u32,
    pub load_ms: u64,
    pub dominant: f64,
    pub blank: bool,
    /// Страница не дозагрузилась; кадр — то, что успело нарисоваться.
    #[serde(default)]
    pub partial: bool,
    pub title: Option<String>,
    pub final_url: Option<String>,
    pub console: Vec<String>,
    pub console_total: usize,
}

#[cfg(not(feature = "servo"))]
pub fn run(_args: Args) -> i32 {
    eprintln!("nrb собран без движка: пересоберите с `--features servo`");
    64
}

#[cfg(feature = "servo")]
pub fn run(args: Args) -> i32 {
    let code = run_inner(args);
    nrb_engine_servo::cleanup_ephemeral();
    code
}

#[cfg(feature = "servo")]
fn run_inner(args: Args) -> i32 {
    use nrb_engine::{EngineError, SnapshotRequest, Snapshotter};
    use nrb_engine_servo::{ServoConfig, ServoSnapshotter};

    // Туннель живёт до конца процесса: движок ходит в сеть только через него.
    if args.node.hermetic && !args.node.enabled() {
        eprintln!("герметичный режим требует узла (--node)");
        return 3;
    }
    let tunnel = if args.node.enabled() {
        match crate::tunnel::start(&args.node).and_then(|mut tunnel| {
            tunnel.isolate(args.node.hermetic)?; // герметичный режим: дальше потоки движка в пустом сетевом пространстве имён
            Ok(tunnel)
        }) {
            Ok(tunnel) => Some(tunnel),
            Err(error) => {
                eprintln!("туннель: {error}");
                return 3;
            }
        }
    } else {
        None
    };
    let network_connector = tunnel.as_ref().map(|t| t.engine_connector());
    let proxy = if network_connector.is_some() {
        None
    } else {
        args.proxy
    };
    let engine = match ServoSnapshotter::new(&ServoConfig {
        proxy,
        network_connector,
        user_agent: std::env::var("NRB_USER_AGENT")
            .ok()
            .filter(|s| !s.is_empty()),
        profile_dir: args.profile.for_snap(),
        devtools_port: None,
        // Отладка: `NRB_DEBUG_SCRIPT=файл.js` внедряется во все страницы снимка (см. tests/debug/error-hook.js).
        user_scripts: std::env::var_os("NRB_DEBUG_SCRIPT")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .into_iter()
            .collect(),
    }) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("не удалось запустить движок: {error:#}");
            return 3;
        }
    };
    // Servo при штатной остановке (Drop) может упасть с abort на assert внутри WebGL (`webglbuffer.rs: is_deleted`; на
    // release это роняет процесс уже после готового снимка и теряет код выхода). Процесс всё равно завершается — не
    // разбираем движок.
    // `NRB_GRACEFUL_EXIT=1` возвращает штатную остановку (для отладки самих падений Servo при выходе).
    let mut engine = TeardownGuard {
        inner: std::mem::ManuallyDrop::new(engine),
        graceful: std::env::var_os("NRB_GRACEFUL_EXIT").is_some(),
    };
    let request = SnapshotRequest {
        url: args.url,
        size: args.size,
        timeout: args.timeout,
        settle: args.settle,
    };
    match engine.snapshot(&request) {
        Ok(snapshot) => {
            if let Err(error) = snapshot.frame.write_png(&args.output) {
                eprintln!("не удалось записать {}: {error}", args.output.display());
                return 3;
            }
            let ratio = snapshot.frame.dominant_color_ratio();
            if args.json {
                let report = SnapReport {
                    width: snapshot.frame.size.width,
                    height: snapshot.frame.size.height,
                    load_ms: snapshot.load_time.as_millis() as u64,
                    dominant: ratio,
                    blank: snapshot.frame.is_blank(),
                    partial: snapshot.partial,
                    title: snapshot.title.clone(),
                    final_url: snapshot.final_url.as_ref().map(|u| u.to_string()),
                    console: snapshot.console.iter().take(20).cloned().collect(),
                    console_total: snapshot.console.len(),
                };
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("сериализация отчёта")
                );
            } else {
                println!(
                    "{} {}x{} load={}ms dominant={:.3} console={} -> {}",
                    if snapshot.partial { "partial" } else { "ok" },
                    snapshot.frame.size.width,
                    snapshot.frame.size.height,
                    snapshot.load_time.as_millis(),
                    ratio,
                    snapshot.console.len(),
                    args.output.display()
                );
            }
            if snapshot.frame.is_blank() {
                eprintln!("кадр пустой (один цвет занимает {:.1}%)", ratio * 100.0);
                return 4;
            }
            0
        }
        Err(EngineError::Timeout(after)) => {
            eprintln!("таймаут {after:?}");
            2
        }
        Err(error) => {
            eprintln!("{error}");
            3
        }
    }
}

/// Держит движок так, чтобы он не разбирался при выходе (см. комментарий у создания), если не просили иного.
#[cfg(feature = "servo")]
struct TeardownGuard {
    inner: std::mem::ManuallyDrop<nrb_engine_servo::ServoSnapshotter>,
    graceful: bool,
}

#[cfg(feature = "servo")]
impl Drop for TeardownGuard {
    fn drop(&mut self) {
        if self.graceful {
            // SAFETY: `inner` больше не используется после этого вызова.
            unsafe { std::mem::ManuallyDrop::drop(&mut self.inner) }
        }
    }
}

#[cfg(feature = "servo")]
impl std::ops::Deref for TeardownGuard {
    type Target = nrb_engine_servo::ServoSnapshotter;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(feature = "servo")]
impl std::ops::DerefMut for TeardownGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
