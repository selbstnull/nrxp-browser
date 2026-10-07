//! `nrb fetch` — загрузка URL через узел NRXP (I4): проверка транспорта без движка.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nrb_net::Fetcher;
use nrb_transport::{NodeConfig, NrxpTransport, SystemTransport, Transport};
use url::Url;

pub struct Args {
    pub url: Url,
    pub node: Option<PathBuf>,
    pub node_addr: Option<String>,
    pub system: bool,
    pub output: Option<PathBuf>,
    pub include: bool,
    pub timeout: Duration,
}

pub fn run(args: Args) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("не удалось запустить рантайм: {error}");
            return 3;
        }
    };
    runtime.block_on(run_async(args))
}

async fn transport(args: &Args) -> Result<Arc<dyn Transport>, String> {
    if args.system {
        return Ok(Arc::new(SystemTransport));
    }
    let config = match (&args.node, &args.node_addr) {
        (Some(link), _) if link.to_string_lossy().starts_with("nrxp://") => {
            nrb_node::Invite::parse(&link.to_string_lossy())
                .map_err(|e| e.to_string())?
                .to_node_config()
        }
        (Some(path), _) => NodeConfig::from_toml_file(path).map_err(|e| e.to_string())?,
        (None, Some(addr)) => NodeConfig::anonymous(addr.clone()),
        (None, None) => NodeConfig::from_env().map_err(|e| {
            format!("{e} (укажите --node, --node-addr, --system или переменные NRB_NODE_*)")
        })?,
    };
    let transport = NrxpTransport::connect(&config)
        .await
        .map_err(|e| format!("подключение к узлу {}: {e}", config.addr))?;
    Ok(Arc::new(transport))
}

async fn run_async(args: Args) -> i32 {
    let transport = match transport(&args).await {
        Ok(transport) => transport,
        Err(error) => {
            eprintln!("{error}");
            return 3;
        }
    };
    let name = transport.name();
    let fetcher = Fetcher::new(transport).with_timeout(args.timeout);
    let started = std::time::Instant::now();

    // Тело пишем потоково (в файл или stdout) — память не растёт с размером ответа.
    let mut writer: Box<dyn Write + Send> = match &args.output {
        Some(path) => match std::fs::File::create(path) {
            Ok(file) => Box::new(std::io::BufWriter::new(file)),
            Err(error) => {
                eprintln!("не удалось создать {}: {error}", path.display());
                return 3;
            }
        },
        None => Box::new(std::io::stdout()),
    };
    let mut total = 0u64;
    let mut sink = |chunk: &[u8]| {
        total += chunk.len() as u64;
        writer.write_all(chunk)
    };
    let response = match fetcher.get_streaming(&args.url, &mut sink).await {
        Ok(response) => response,
        Err(error) => {
            eprintln!("ошибка: {error}");
            return 2;
        }
    };
    if let Err(error) = writer.flush() {
        eprintln!("не удалось записать результат: {error}");
        return 3;
    }
    if args.include {
        eprintln!(
            "HTTP {} {} через {name}, редиректов: {}, итоговый URL: {}",
            response.version, response.status, response.redirects, response.final_url
        );
        for (key, value) in &response.headers {
            eprintln!("{key}: {value}");
        }
        eprintln!();
    }
    eprintln!("{total} байт за {:?}", started.elapsed());
    if (200..400).contains(&response.status) {
        0
    } else {
        1
    }
}
