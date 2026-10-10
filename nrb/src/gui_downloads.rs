//! `DownloadHandler` для окна «Загрузки» (N3a): загрузчик `nrb-net` поверх того же транспорта, что и страницы (туннель NRXP,
//! в герметичном режиме — тоже через рантайм туннеля, у потока окна сети нет).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nrb_net::Fetcher;
use nrb_net::download::{BodyEvent, DownloadRequest, DownloadState, Downloads, IncomingDownload};
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

/// Каталог загрузок (правила по ОС — `platform::downloads_dir`).
pub fn downloads_dir() -> PathBuf {
    crate::platform::downloads_dir()
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
    fn ingest(
        &self,
        incoming: nrb_shell::IncomingDownload,
        events: Box<dyn Iterator<Item = nrb_shell::BodyEvent> + Send>,
    ) {
        self.downloads.ingest(
            IncomingDownload {
                url: incoming.url,
                suggested_name: incoming.suggested_name,
                mime: incoming.mime,
                total: incoming.total,
                dest_dir: self.dir.clone(),
            },
            events.map(|event| match event {
                nrb_shell::BodyEvent::Chunk(chunk) => BodyEvent::Chunk(chunk),
                nrb_shell::BodyEvent::Done => BodyEvent::Done,
                nrb_shell::BodyEvent::Failed(reason) => BodyEvent::Failed(reason),
            }),
        );
    }

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

    fn reserve_file(&self, suggested_name: &str) -> Option<PathBuf> {
        nrb_net::download::reserve_unique_path(&self.dir, suggested_name).ok()
    }

    fn open_dir(&self) {
        let _ = std::fs::create_dir_all(&self.dir);
        crate::platform::open_path(&self.dir);
    }
}
