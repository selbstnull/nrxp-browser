use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use nrb_engine::Size;
use url::Url;

mod adblock;
mod corpus;
mod deploy;
mod fetch;
mod gui;
#[cfg(feature = "servo")]
mod gui_nodes;
mod hermetic;
mod node;
mod profile;
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
    #[command(flatten)]
    profile: profile::ProfileArgs,
    /// Включить сервер DevTools Servo на 127.0.0.1:ПОРТ (подключение — из Firefox → about:debugging).
    #[arg(long, value_name = "ПОРТ")]
    devtools: Option<u16>,
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
        #[command(flatten)]
        profile: profile::ProfileArgs,
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
    /// Свой узел NRXP: создать, запустить, выдать приглашение устройству, отозвать устройство.
    Node {
        #[command(subcommand)]
        command: node::NodeCommand,
    },
    /// Поставить свой узел на сервер по SSH и получить приглашение (откат при сбое, повторный запуск = апгрейд).
    Deploy(deploy::DeployArgs),
    /// Спариться с чужим узлом по приглашению `nrxp://…` (сохраняет конфиг для `--node`).
    Pair {
        /// Ссылка-приглашение, выданная `nrb node invite`.
        link: String,
        /// Куда сохранить конфиг (по умолчанию `~/.local/share/netrunner-browser/nodes/<имя>.toml`).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Стать выходом для чужого узла (reverse-egress): дозвониться до ingress и обслуживать его клиентов. Работает за NAT.
    Egress {
        /// Приглашение `nrxp://…` (из `nrb node invite --egress`) или путь к конфигу узла.
        node: String,
        /// Подпись в реестре узла (по умолчанию — имя из приглашения).
        #[arg(long)]
        label: Option<String>,
        /// Разрешить цели в локальной сети этого устройства (127.0.0.1, 192.168.*, …). По умолчанию запрещено.
        #[arg(long)]
        allow_lan: bool,
    },
    /// Блокировщик рекламы: скачать списки фильтров, показать состояние.
    Adblock {
        #[command(subcommand)]
        command: adblock::AdblockCommand,
    },
    /// Самопроверка герметичного режима (I11): прямые соединения невозможны, через релей и туннель — работает.
    HermeticSelftest {
        #[command(flatten)]
        node: tunnel::NodeArgs,
        /// Порт TCP-слушателя на loopback хоста: из герметичного режима он обязан быть недоступен.
        #[arg(long)]
        host_port: Option<u16>,
    },
    /// Версия.
    Version,
}

fn main() {
    // До потоков: пространство имён пользователя можно создать только в однопоточном процессе.
    hermetic::early_init();
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
            profile,
        }) => snap::run(snap::Args {
            url,
            output,
            size,
            timeout,
            settle,
            proxy,
            json,
            node,
            profile,
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
        Some(Command::Node { command }) => node::run(command),
        Some(Command::Deploy(args)) => deploy::run(args),
        Some(Command::Pair { link, out }) => node::pair(&link, out),
        Some(Command::Egress {
            node,
            label,
            allow_lan,
        }) => node::egress(&node, label, allow_lan),
        Some(Command::Adblock { command }) => adblock::run(command),
        Some(Command::HermeticSelftest { node, host_port }) => hermetic::selftest(&node, host_port),
        Some(Command::Version) => {
            println!("nrb {}", env!("CARGO_PKG_VERSION"));
            0
        }
        None => gui::run(cli.urls, cli.proxy, cli.node, cli.profile, cli.devtools),
    };
    std::process::exit(code);
}
