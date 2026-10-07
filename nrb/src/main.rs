use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use nrb_engine::Size;
use url::Url;

mod gui;
mod snap;

#[derive(Parser)]
#[command(
    name = "nrb",
    version,
    about = "netrunner-browser",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Адреса или поисковые запросы для открытия во вкладках (без подкоманды запускается окно).
    urls: Vec<String>,
    /// Прокси для движка (HTTP CONNECT), например http://127.0.0.1:3128.
    #[arg(long)]
    proxy: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Загрузить страницу и сохранить скриншот (headless). Коды: 0 ок, 2 таймаут, 3 крах/ошибка движка, 4 пустой кадр, 64 собрано без движка.
    Snap {
        url: Url,
        /// Куда сохранить PNG.
        #[arg(short, long)]
        output: PathBuf,
        /// Размер вьюпорта.
        #[arg(long, default_value = "1280x800")]
        size: Size,
        /// Максимальное время на загрузку (например 30s).
        #[arg(long, default_value = "30s", value_parser = humantime::parse_duration)]
        timeout: Duration,
        /// Дополнительная пауза после полной загрузки.
        #[arg(long, default_value = "500ms", value_parser = humantime::parse_duration)]
        settle: Duration,
        /// Прокси для движка, например http://127.0.0.1:3128.
        #[arg(long)]
        proxy: Option<String>,
    },
    /// Версия.
    Version,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Snap {
            url,
            output,
            size,
            timeout,
            settle,
            proxy,
        }) => snap::run(snap::Args {
            url,
            output,
            size,
            timeout,
            settle,
            proxy,
        }),
        Some(Command::Version) => {
            println!("nrb {}", env!("CARGO_PKG_VERSION"));
            0
        }
        None => gui::run(cli.urls, cli.proxy),
    };
    std::process::exit(code);
}
