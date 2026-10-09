//! Загрузки файлов (N3): потоковая запись на диск через [`Fetcher`] (то есть через тот же транспорт, что и страницы — туннель NRXP),
//! с именем файла из `Content-Disposition`/URL, безопасным именем, `.part` до завершения и отменой.
//!
//! Безопасность имени: из сервера приходит произвольная строка — берётся только последний компонент пути, управляющие символы и
//! `/ \ : * ? " < > |` заменяются, ведущие точки и пробелы срезаются, длина ограничена; существующие файлы не перезаписываются
//! (`имя (1).ext`).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use url::Url;

use crate::{FetchError, Fetcher, RequestOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct DownloadInfo {
    pub id: u64,
    /// Хост источника (для показа; полный URL может содержать секреты в строке запроса).
    pub source_host: String,
    pub file_name: String,
    /// Итоговый путь (появляется после получения заголовка ответа).
    pub path: Option<PathBuf>,
    pub received: u64,
    pub total: Option<u64>,
    pub state: DownloadState,
}

#[derive(Debug, Clone, Default)]
pub struct DownloadRequest {
    pub url: Option<Url>,
    pub dest_dir: PathBuf,
    /// Значение заголовка `Cookie` (из хранилища браузера) и `Referer` страницы-источника.
    pub cookies: Option<String>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Default)]
pub struct Downloads {
    list: Mutex<Vec<DownloadInfo>>,
    cancel: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    next_id: AtomicU64,
}

impl Downloads {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn list(&self) -> Vec<DownloadInfo> {
        self.list.lock().expect("mutex загрузок").clone()
    }

    pub fn cancel(&self, id: u64) {
        if let Some(flag) = self.cancel.lock().expect("mutex загрузок").get(&id) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Убирает из списка завершённые/упавшие/отменённые.
    pub fn clear_finished(&self) {
        self.list
            .lock()
            .expect("mutex загрузок")
            .retain(|d| d.state == DownloadState::Running);
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut DownloadInfo)) {
        if let Some(info) = self
            .list
            .lock()
            .expect("mutex загрузок")
            .iter_mut()
            .find(|d| d.id == id)
        {
            f(info);
        }
    }

    /// Запускает загрузку на `handle` (рантайм, где живёт транспорт). Возвращает идентификатор.
    pub fn start(
        self: &Arc<Self>,
        fetcher: Arc<Fetcher>,
        handle: &tokio::runtime::Handle,
        request: DownloadRequest,
    ) -> Option<u64> {
        let url = request.url.clone()?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancel
            .lock()
            .expect("mutex загрузок")
            .insert(id, cancelled.clone());
        self.list
            .lock()
            .expect("mutex загрузок")
            .push(DownloadInfo {
                id,
                source_host: url.host_str().unwrap_or("").to_owned(),
                file_name: file_name_from(None, &url, None),
                path: None,
                received: 0,
                total: None,
                state: DownloadState::Running,
            });
        let this = self.clone();
        handle.spawn(async move {
            let outcome = this
                .run(id, &fetcher, &url, &request, cancelled.clone())
                .await;
            let state = match outcome {
                Ok(()) => DownloadState::Done,
                Err(_) if cancelled.load(Ordering::SeqCst) => DownloadState::Cancelled,
                Err(error) => DownloadState::Failed(error),
            };
            this.update(id, |info| info.state = state);
            this.cancel.lock().expect("mutex загрузок").remove(&id);
        });
        Some(id)
    }

    async fn run(
        self: &Arc<Self>,
        id: u64,
        fetcher: &Fetcher,
        url: &Url,
        request: &DownloadRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mut options = RequestOptions {
            user_agent: request.user_agent.clone(),
            ..Default::default()
        };
        if let Some(cookies) = &request.cookies {
            options.headers.push(("cookie".into(), cookies.clone()));
        }
        if let Some(referer) = &request.referer {
            options.headers.push(("referer".into(), referer.clone()));
        }

        // Файл создаётся в `on_head` (когда известно имя) и пишется в `sink`; общее состояние — через Mutex.
        let file: Arc<Mutex<Option<(std::fs::File, PathBuf, PathBuf)>>> =
            Arc::new(Mutex::new(None));
        let (file_head, file_sink) = (file.clone(), file.clone());
        let (this_head, this_sink) = (self.clone(), self.clone());
        let dest_dir = request.dest_dir.clone();
        let cancelled_sink = cancelled.clone();
        let mut received = 0u64;

        let mut on_head = move |head: &crate::ResponseHead<'_>| -> std::io::Result<()> {
            if !(200..300).contains(&head.status) {
                return Err(std::io::Error::other(format!(
                    "сервер ответил {}",
                    head.status
                )));
            }
            let name = file_name_from(
                head.header("content-disposition"),
                head.final_url,
                head.header("content-type"),
            );
            std::fs::create_dir_all(&dest_dir)?;
            let (final_path, part_path, file) = create_unique(&dest_dir, &name)?;
            let total = head
                .header("content-length")
                .and_then(|v| v.trim().parse::<u64>().ok());
            this_head.update(id, |info| {
                info.file_name = final_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(name.clone());
                info.path = Some(final_path.clone());
                info.total = total;
            });
            *file_head.lock().expect("mutex файла") = Some((file, final_path, part_path));
            Ok(())
        };
        let mut sink = move |chunk: &[u8]| -> std::io::Result<()> {
            if cancelled_sink.load(Ordering::SeqCst) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "отменено",
                ));
            }
            let mut guard = file_sink.lock().expect("mutex файла");
            let Some((file, _, _)) = guard.as_mut() else {
                return Err(std::io::Error::other("нет открытого файла"));
            };
            file.write_all(chunk)?;
            received += chunk.len() as u64;
            this_sink.update(id, |info| info.received = received);
            Ok(())
        };

        let result = fetcher
            .download(url, &options, &mut on_head, &mut sink)
            .await;
        let taken = file.lock().expect("mutex файла").take();
        match (result, taken) {
            (Ok(_), Some((mut file, final_path, part_path))) => {
                file.flush().map_err(|e| e.to_string())?;
                drop(file);
                std::fs::rename(&part_path, &final_path).map_err(|e| e.to_string())?;
                Ok(())
            }
            (Ok(_), None) => Err("сервер не вернул тела".into()),
            (Err(error), taken) => {
                if let Some((file, _, part_path)) = taken {
                    drop(file);
                    let _ = std::fs::remove_file(part_path);
                }
                Err(match error {
                    FetchError::Http(message) => message,
                    other => other.to_string(),
                })
            }
        }
    }
}

/// Имя файла: `Content-Disposition` (`filename*` важнее `filename`), иначе последний сегмент пути URL, иначе `download`;
/// без расширения — по `Content-Type` для самых частых типов.
pub fn file_name_from(
    content_disposition: Option<&str>,
    url: &Url,
    content_type: Option<&str>,
) -> String {
    let from_header = content_disposition.and_then(parse_content_disposition);
    let from_url = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .map(percent_decode);
    let mut name = sanitize_file_name(&from_header.or(from_url).unwrap_or_default());
    if !name.contains('.') {
        if let Some(ext) = content_type.and_then(extension_for) {
            name.push('.');
            name.push_str(ext);
        }
    }
    name
}

fn extension_for(content_type: &str) -> Option<&'static str> {
    let essence = content_type.split(';').next()?.trim().to_ascii_lowercase();
    Some(match essence.as_str() {
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/json" => "json",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "audio/mpeg" => "mp3",
        "video/mp4" => "mp4",
        _ => return None,
    })
}

fn parse_content_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    for part in value.split(';').skip(1) {
        let Some((key, raw)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let raw = raw.trim();
        if key == "filename*" {
            // RFC 5987: charset'lang'percent-encoded; поддерживаем UTF-8 (остальное — как есть).
            let encoded = raw.rsplit('\'').next().unwrap_or(raw);
            return Some(percent_decode(encoded));
        }
        if key == "filename" {
            plain = Some(raw.trim_matches('"').replace("\\\"", "\""));
        }
    }
    plain
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len() + 0
            && let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Безопасное имя файла (см. описание модуля).
pub fn sanitize_file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut out = String::new();
    for c in trimmed.chars() {
        if out.len() + c.len_utf8() > 200 {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "download".to_owned()
    } else {
        out
    }
}

/// Создаёт `<dir>/<имя>.part` и резервирует свободное итоговое имя (`имя (1).ext`, …), не затирая существующее.
fn create_unique(dir: &Path, name: &str) -> std::io::Result<(PathBuf, PathBuf, std::fs::File)> {
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    for n in 0..1000u32 {
        let candidate = if n == 0 {
            name.to_owned()
        } else {
            format!("{stem} ({n}){ext}")
        };
        let final_path = dir.join(&candidate);
        let part_path = dir.join(format!("{candidate}.part"));
        if final_path.exists() {
            continue;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
        {
            Ok(file) => return Ok((final_path, part_path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "не удалось подобрать свободное имя файла",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        s.parse().unwrap()
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("..\\..\\win.ini"), "win.ini");
        assert_eq!(sanitize_file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_file_name(".hidden"), "hidden");
        assert_eq!(sanitize_file_name("   "), "download");
        assert_eq!(sanitize_file_name(""), "download");
        assert_eq!(sanitize_file_name("..."), "download");
        assert!(sanitize_file_name(&"я".repeat(500)).len() <= 200);
        assert_eq!(sanitize_file_name("tab\tname.txt"), "tab_name.txt");
    }

    #[test]
    fn content_disposition_variants() {
        let u = url("https://x.test/path/fallback.bin");
        assert_eq!(
            file_name_from(Some("attachment; filename=\"report 1.pdf\""), &u, None),
            "report 1.pdf"
        );
        assert_eq!(
            file_name_from(Some("attachment; filename=plain.txt"), &u, None),
            "plain.txt"
        );
        // filename* важнее filename и раскодируется
        assert_eq!(
            file_name_from(
                Some(
                    "attachment; filename=\"fallback.txt\"; filename*=UTF-8''%D0%BE%D1%82%D1%87%D1%91%D1%82.pdf"
                ),
                &u,
                None
            ),
            "отчёт.pdf"
        );
        // попытка выйти из каталога
        assert_eq!(
            file_name_from(Some("attachment; filename=\"../../evil.sh\""), &u, None),
            "evil.sh"
        );
    }

    #[test]
    fn name_from_url_and_content_type() {
        assert_eq!(
            file_name_from(
                None,
                &url("https://x.test/a/b/data%20set.csv?token=1"),
                None
            ),
            "data set.csv"
        );
        assert_eq!(
            file_name_from(None, &url("https://x.test/"), None),
            "download"
        );
        assert_eq!(
            file_name_from(
                None,
                &url("https://x.test/get"),
                Some("application/pdf; charset=binary")
            ),
            "get.pdf"
        );
    }

    #[test]
    fn unique_names_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("nrb-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "old").unwrap();
        let (final_path, part, _f) = create_unique(&dir, "a.txt").unwrap();
        assert_eq!(final_path.file_name().unwrap(), "a (1).txt");
        assert_eq!(part.file_name().unwrap(), "a (1).txt.part");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "old");
        let (second, _, _g) = create_unique(&dir, "a.txt").unwrap();
        assert_eq!(second.file_name().unwrap(), "a (2).txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
