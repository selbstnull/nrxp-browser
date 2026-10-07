//! `nrb snap` — headless-снимок страницы (I1).

use std::path::PathBuf;
use std::time::Duration;

use nrb_engine::Size;
use url::Url;

pub struct Args {
    pub url: Url,
    pub output: PathBuf,
    pub size: Size,
    pub timeout: Duration,
    pub settle: Duration,
    pub proxy: Option<String>,
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

    let mut engine = match ServoSnapshotter::new(&ServoConfig { proxy: args.proxy, user_agent: None }) {
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
            println!(
                "ok {}x{} load={}ms dominant={:.3} console={} -> {}",
                snapshot.frame.size.width,
                snapshot.frame.size.height,
                snapshot.load_time.as_millis(),
                ratio,
                snapshot.console.len(),
                args.output.display()
            );
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
