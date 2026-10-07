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
    use nrb_engine::{EngineError, SnapshotRequest, Snapshotter};
    use nrb_engine_servo::{ServoConfig, ServoSnapshotter};

    // Туннель живёт до конца процесса: движок ходит в сеть только через него.
    let tunnel = if args.node.enabled() {
        match crate::tunnel::start(&args.node) {
            Ok(tunnel) => Some(tunnel),
            Err(error) => {
                eprintln!("туннель: {error}");
                return 3;
            }
        }
    } else {
        None
    };
    let proxy = tunnel.as_ref().map(|t| t.proxy_url()).or(args.proxy);
    let mut engine = match ServoSnapshotter::new(&ServoConfig {
        proxy,
        user_agent: None,
        profile_dir: args.profile.for_snap(),
        devtools_port: None,
    }) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("не удалось запустить движок: {error:#}");
            return 3;
        }
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
