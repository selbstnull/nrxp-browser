use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use nrb_engine::Size;
use url::Url;

mod corpus;
mod fetch;
mod gui;
mod snap;
mod tunnel;

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
    #[arg(long, env = "NRB_PROXY_URL", hide_env_values = true, conflicts_with_all = ["node", "node_addr"])]
    proxy: Option<String>,
    #[command(flatten)]
    node: tunnel::NodeArgs,
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
        #[arg(long, env = "NRB_PROXY_URL", hide_env_values = true, conflicts_with_all = ["node", "node_addr"])]
        proxy: Option<String>,
        /// Машиночитаемый отчёт (JSON в одну строку) в stdout.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        node: tunnel::NodeArgs,
    },
    /// Прогнать корпус сайтов (по одному процессу `nrb snap` на сайт) и собрать отчёт совместимости.
    Corpus {
        /// Файл со списком сайтов.
        #[arg(long, default_value = "tests/corpus/sites.toml")]
        sites: PathBuf,
        /// Файл с известными нерабочими сайтами.
        #[arg(long, default_value = "tests/corpus/known-broken.toml")]
        known_broken: PathBuf,
        /// Файл baseline (ratchet): прогон падает, если процент ниже.
        #[arg(long, default_value = "tests/corpus/baseline.json")]
        baseline: PathBuf,
        /// Каталог отчёта; по умолчанию out/corpus/<время>.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Только эти категории (через запятую).
        #[arg(long, value_delimiter = ',')]
        category: Vec<String>,
        /// Только сайты, id которых содержит эту подстроку.
        #[arg(long)]
        filter: Option<String>,
        /// Сколько сайтов обрабатывать одновременно.
        #[arg(long, default_value_t = 2)]
        parallel: usize,
        /// Таймаут одного сайта.
        #[arg(long, default_value = "30s", value_parser = humantime::parse_duration)]
        timeout: Duration,
        /// Прокси для движка (передаётся каждому `nrb snap` через переменную окружения).
        #[arg(long, conflicts_with_all = ["node", "node_addr"])]
        proxy: Option<String>,
        #[command(flatten)]
        node: tunnel::NodeArgs,
        /// Не падать из-за baseline (только отчёт).
        #[arg(long)]
        no_ratchet: bool,
    },
    /// Скачать URL (GET, h1/h2) через узел NRXP или напрямую: диагностика транспорта без движка.
    Fetch {
        url: Url,
        /// Конфиг узла (TOML: addr, decoy_sni, nrxp_secret, nrxp_public_key, auth_token).
        #[arg(long, conflicts_with_all = ["node_addr", "system"])]
        node: Option<PathBuf>,
        /// Анонимный узел `host:port` (только для локальной разработки).
        #[arg(long, conflicts_with = "system")]
        node_addr: Option<String>,
        /// Обычные сокеты ОС вместо NRXP (DNS и трафик мимо туннеля — только для сравнения).
        #[arg(long)]
        system: bool,
        /// Куда записать тело; по умолчанию — stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Печатать статус и заголовки в stderr.
        #[arg(short = 'i', long)]
        include: bool,
        #[arg(long, default_value = "60s", value_parser = humantime::parse_duration)]
        timeout: Duration,
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
            json,
            node,
        }) => snap::run(snap::Args {
            url,
            output,
            size,
            timeout,
            settle,
            proxy,
            json,
            node,
        }),
        Some(Command::Corpus {
            sites,
            known_broken,
            baseline,
            out,
            category,
            filter,
            parallel,
            timeout,
            proxy,
            node,
            no_ratchet,
        }) => corpus::run(corpus::Args {
            sites,
            known_broken,
            baseline,
            out,
            category,
            filter,
            parallel,
            timeout,
            proxy,
            node,
            no_ratchet,
        }),
        Some(Command::Fetch {
            url,
            node,
            node_addr,
            system,
            output,
            include,
            timeout,
        }) => fetch::run(fetch::Args {
            url,
            node,
            node_addr,
            system,
            output,
            include,
            timeout,
        }),
        Some(Command::Version) => {
            println!("nrb {}", env!("CARGO_PKG_VERSION"));
            0
        }
        None => gui::run(cli.urls, cli.proxy, cli.node),
    };
    std::process::exit(code);
}
