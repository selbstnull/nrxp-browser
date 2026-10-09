//! `DownloadHandler` для окна «Загрузки» (N3a): загрузчик `nrb-net` поверх того же транспорта, что и страницы (туннель NRXP,
//! в герметичном режиме — тоже через рантайм туннеля, у потока окна сети нет).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nrb_net::Fetcher;
use nrb_net::download::{DownloadRequest, DownloadState, Downloads};
use nrb_shell::{DownloadHandler, DownloadRow, RowState};
use nrb_transport::Transport;
use url::Url;

pub struct Handler {
    downloads: Arc<Downloads>,
    fetcher: Arc<Fetcher>,
    handle: tokio::runtime::Handle,
    dir: PathBuf,
    /// Владеет рантаймом, если своего у транспорта нет (прямая загрузка без узла).
    _runtime: Option<tokio::runtime::Runtime>,
}

/// Каталог загрузок: `XDG_DOWNLOAD_DIR` из `~/.config/user-dirs.dirs`, иначе `~/Downloads`.
pub fn downloads_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    if let Ok(text) = std::fs::read_to_string(config.join("user-dirs.dirs")) {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") {
                let value = value
                    .trim()
                    .trim_matches('"')
                    .replace("$HOME", &home.to_string_lossy());
                let path = PathBuf::from(value);
                if path.is_absolute() && path != home {
                    return path;
                }
            }
        }
    }
    home.join("Downloads")
}

impl Handler {
    /// С транспортом туннеля: `handle` — рантайм туннеля.
    pub fn with_transport(
        transport: Arc<dyn Transport>,
        handle: tokio::runtime::Handle,
        runtime: Option<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            downloads: Downloads::new(),
            // Таймаут — на всю загрузку целиком: для файлов большим.
            fetcher: Arc::new(Fetcher::new(transport).with_timeout(Duration::from_secs(6 * 3600))),
            handle,
            dir: downloads_dir(),
            _runtime: runtime,
        }
    }

    /// Прямая загрузка (без узла) со своим небольшим рантаймом.
    pub fn direct() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nrb-downloads")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let handle = runtime.handle().clone();
        Ok(Self::with_transport(
            Arc::new(nrb_transport::SystemTransport),
            handle,
            Some(runtime),
        ))
    }
}

impl DownloadHandler for Handler {
    fn start(&self, url: &Url, cookies: Option<String>, referer: Option<String>) {
        self.downloads.start(
            self.fetcher.clone(),
            &self.handle,
            DownloadRequest {
                url: Some(url.clone()),
                dest_dir: self.dir.clone(),
                cookies,
                referer,
                user_agent: None,
            },
        );
    }

    fn list(&self) -> Vec<DownloadRow> {
        self.downloads
            .list()
            .into_iter()
            .map(|d| DownloadRow {
                id: d.id,
                file_name: d.file_name,
                source_host: d.source_host,
                received: d.received,
                total: d.total,
                state: match d.state {
                    DownloadState::Running => RowState::Running,
                    DownloadState::Done => RowState::Done,
                    DownloadState::Failed(e) => RowState::Failed(e),
                    DownloadState::Cancelled => RowState::Cancelled,
                },
            })
            .collect()
    }

    fn cancel(&self, id: u64) {
        self.downloads.cancel(id);
    }

    fn clear_finished(&self) {
        self.downloads.clear_finished();
    }

    fn dir(&self) -> String {
        self.dir.display().to_string()
    }

    fn open_dir(&self) {
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::process::Command::new("xdg-open")
            .arg(&self.dir)
            .spawn();
    }
}
