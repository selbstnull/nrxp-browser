//! HTTP-клиент браузера поверх [`Transport`]: h1/h2 с TLS (корни webpki зашиты в бинарь — системное хранилище не нужно).
//! Сейчас это диагностический `nrb fetch`; кэш, куки, HSTS и h3 добавляются здесь же (I10, I12).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper::header::HeaderValue;
use hyper_util::rt::{TokioExecutor, TokioIo};
use nrb_transport::{BoxStream, Transport};
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;
use url::Url;

const MAX_REDIRECTS: usize = 10;
const MAX_BODY: usize = 256 * 1024 * 1024;
const USER_AGENT: &str = concat!("nrb/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("неподдерживаемая схема `{0}` (нужны http/https)")]
    Scheme(String),
    #[error("в адресе нет хоста")]
    NoHost,
    #[error("соединение с {host}:{port}: {source}")]
    Connect {
        host: String,
        port: u16,
        source: std::io::Error,
    },
    #[error("TLS: {0}")]
    Tls(String),
    #[error("HTTP: {0}")]
    Http(String),
    #[error("слишком много редиректов (> {MAX_REDIRECTS})")]
    TooManyRedirects,
    #[error("таймаут {0:?}")]
    Timeout(Duration),
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    /// `h2` или `http/1.1`.
    pub version: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub final_url: Url,
    pub redirects: usize,
}

/// Куда потоково складывать тело: вызывается на каждый кусок, ошибка прерывает загрузку.
pub type BodySink<'a> = &'a mut (dyn FnMut(&[u8]) -> std::io::Result<()> + Send);

struct Head {
    status: u16,
    version: &'static str,
    headers: Vec<(String, String)>,
    body: hyper::body::Incoming,
}

pub struct Fetcher {
    transport: Arc<dyn Transport>,
    tls: TlsConnector,
    timeout: Duration,
}

fn tls_config() -> rustls::ClientConfig {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    // Провайдер задаём явно: в процессе одновременно живут aws-lc (Servo) и ring (ядро) — полагаться на «провайдер по умолчанию» нельзя.
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("версии протоколов TLS")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    config
}

impl Fetcher {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            tls: TlsConnector::from(Arc::new(tls_config())),
            timeout: Duration::from_secs(60),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// GET с редиректами. Тело читается целиком в память (до 256 МБ).
    pub async fn get(&self, url: &Url) -> Result<Response, FetchError> {
        tokio::time::timeout(self.timeout, self.get_inner(url, None))
            .await
            .map_err(|_| FetchError::Timeout(self.timeout))?
    }

    /// GET с редиректами; тело (последнего ответа) уходит в `sink` кусками, память не растёт.
    /// В возвращённом [`Response`] `body` пуст.
    pub async fn get_streaming(
        &self,
        url: &Url,
        sink: BodySink<'_>,
    ) -> Result<Response, FetchError> {
        tokio::time::timeout(self.timeout, self.get_inner(url, Some(sink)))
            .await
            .map_err(|_| FetchError::Timeout(self.timeout))?
    }

    async fn get_inner(
        &self,
        start: &Url,
        mut sink: Option<BodySink<'_>>,
    ) -> Result<Response, FetchError> {
        let mut url = start.clone();
        for redirects in 0..=MAX_REDIRECTS {
            let head = self.get_once(&url).await?;
            let location = matches!(head.status, 301 | 302 | 303 | 307 | 308)
                .then(|| {
                    head.headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                        .map(|(_, v)| v.clone())
                })
                .flatten();
            if let Some(location) = location {
                url = url.join(&location).map_err(|e| {
                    FetchError::Http(format!("некорректный Location `{location}`: {e}"))
                })?;
                continue;
            }
            let (status, version, headers) = (head.status, head.version, head.headers);
            let mut body = head.body;
            let mut collected = Vec::new();
            let mut total = 0usize;
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|e| FetchError::Http(format!("чтение тела: {e}")))?;
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                total += data.len();
                if sink.is_none() && total > MAX_BODY {
                    return Err(FetchError::Http(format!("тело больше {MAX_BODY} байт")));
                }
                match sink.as_mut() {
                    Some(sink) => {
                        sink(&data).map_err(|e| FetchError::Http(format!("запись тела: {e}")))?
                    }
                    None => collected.extend_from_slice(&data),
                }
            }
            return Ok(Response {
                status,
                version,
                headers,
                body: Bytes::from(collected),
                final_url: url,
                redirects,
            });
        }
        Err(FetchError::TooManyRedirects)
    }

    async fn get_once(&self, url: &Url) -> Result<Head, FetchError> {
        let https = match url.scheme() {
            "https" => true,
            "http" => false,
            other => return Err(FetchError::Scheme(other.into())),
        };
        let host = url.host_str().ok_or(FetchError::NoHost)?;
        let port = url.port_or_known_default().ok_or(FetchError::NoHost)?;
        let tcp: BoxStream = self
            .transport
            .open_tcp(host.trim_start_matches('[').trim_end_matches(']'), port)
            .await
            .map_err(|source| FetchError::Connect {
                host: host.into(),
                port,
                source,
            })?;

        let mut request = Request::builder()
            .method("GET")
            .uri(match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_owned(),
            })
            .header(
                "host",
                url.host_str()
                    .map(|h| match url.port() {
                        Some(p) => format!("{h}:{p}"),
                        None => h.to_owned(),
                    })
                    .unwrap_or_default(),
            )
            .header("user-agent", HeaderValue::from_static(USER_AGENT))
            .header("accept", "*/*")
            // Без сжатия: это диагностический клиент, декодеры не тянем.
            .header("accept-encoding", "identity")
            .body(Empty::<Bytes>::new())
            .map_err(|e| FetchError::Http(e.to_string()))?;

        if https {
            let name = ServerName::try_from(
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_owned(),
            )
            .map_err(|e| FetchError::Tls(e.to_string()))?;
            let tls = self
                .tls
                .connect(name, tcp)
                .await
                .map_err(|e| FetchError::Tls(e.to_string()))?;
            let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
            if h2 {
                // h2 требует :authority вместо Host.
                request.headers_mut().remove("host");
            }
            Self::send(tls, h2, request, url).await
        } else {
            Self::send(tcp, false, request, url).await
        }
    }

    async fn send<T>(
        io: T,
        h2: bool,
        request: Request<Empty<Bytes>>,
        url: &Url,
    ) -> Result<Head, FetchError>
    where
        T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let io = TokioIo::new(io);
        let err = |e: hyper::Error| FetchError::Http(e.to_string());
        let response = if h2 {
            let (mut sender, connection) =
                hyper::client::conn::http2::handshake(TokioExecutor::new(), io)
                    .await
                    .map_err(err)?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            let mut request = request;
            *request.uri_mut() = format!("{}://{}{}", url.scheme(), authority(url), request.uri())
                .parse()
                .map_err(|e| FetchError::Http(format!("{e}")))?;
            sender.send_request(request).await.map_err(err)?
        } else {
            let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
                .await
                .map_err(err)?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            sender.send_request(request).await.map_err(err)?
        };

        let status = response.status().as_u16();
        let version = if h2 { "h2" } else { "http/1.1" };
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        Ok(Head {
            status,
            version,
            headers,
            body: response.into_body(),
        })
    }
}

fn authority(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_owned(),
        _ => String::new(),
    }
}
