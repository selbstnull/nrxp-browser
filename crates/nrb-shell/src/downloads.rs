//! Загрузки файлов в оболочке (N3a): эвристика «этот переход — загрузка», окно «Загрузки» (Ctrl+J).
//!
//! Servo 0.7 не сообщает встраивателю ни о `Content-Disposition`, ни о неотображаемых типах, поэтому пока решаем по расширению
//! пути главного GET-запроса (см. `PLAN.md` форка nrxp-servo, N3b — настоящее решение в движке). Сама загрузка идёт не через
//! Servo, а клиентом `nrb-net` с куками из хранилища браузера.

use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct DownloadRow {
    pub id: u64,
    pub file_name: String,
    pub source_host: String,
    pub received: u64,
    pub total: Option<u64>,
    pub state: RowState,
}

/// Реализует `nrb`: оболочка о транспорте и каталоге загрузок ничего не знает.
pub trait DownloadHandler: Send + Sync {
    /// Начать загрузку. `cookies` — значение заголовка `Cookie`, `referer` — адрес страницы, с которой пошла загрузка.
    fn start(&self, url: &Url, cookies: Option<String>, referer: Option<String>);
    fn list(&self) -> Vec<DownloadRow>;
    fn cancel(&self, id: u64);
    fn clear_finished(&self);
    /// Каталог загрузок (для показа и «Открыть папку»).
    fn dir(&self) -> String;
    fn open_dir(&self);
    /// Свободный путь в каталоге загрузок под файл, который пишет сама оболочка (печать в PDF). Файл уже создан (пустой).
    fn reserve_file(&self, suggested_name: &str) -> Option<std::path::PathBuf>;
}

/// Расширения, которые Servo показать не может (архивы, установщики, документы офиса, PDF, медиа-контейнеры).
const DOWNLOAD_EXTENSIONS: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "iso", "img", "dmg", "exe", "msi",
    "deb", "rpm", "apk", "appimage", "pkg", "jar", "whl", "bin", "pdf", "doc", "docx", "xls",
    "xlsx", "ppt", "pptx", "odt", "ods", "odp", "epub", "mp3", "flac", "wav", "ogg", "mp4", "mkv",
    "avi", "mov", "webm",
];

/// Похожа ли цель главного запроса на файл для загрузки (по расширению последнего сегмента пути).
pub fn looks_like_download(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(segment) = url.path_segments().and_then(|mut s| s.next_back()) else {
        return false;
    };
    let Some((_, ext)) = segment.rsplit_once('.') else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    DOWNLOAD_EXTENSIONS.contains(&ext.as_str())
}

#[derive(Default)]
pub struct DownloadsWindow {
    pub open: bool,
}

fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} ГБ", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} МБ", b as f64 / (1u64 << 20) as f64),
        b => format!("{:.0} КБ", b as f64 / 1024.0),
    }
}

impl DownloadsWindow {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn show(&mut self, ctx: &egui::Context, handler: &dyn DownloadHandler) {
        if !self.open {
            return;
        }
        let mut keep_open = true;
        egui::Window::new("Загрузки")
            .open(&mut keep_open)
            .default_size([560.0, 360.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Каталог: {}", handler.dir()));
                    if ui.button("Открыть папку").clicked() {
                        handler.open_dir();
                    }
                    if ui.button("Очистить список").clicked() {
                        handler.clear_finished();
                    }
                });
                ui.separator();
                let rows = handler.list();
                if rows.is_empty() {
                    ui.weak("Пока пусто. Ссылки на архивы, PDF и т. п. скачиваются сюда.");
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for row in rows.iter().rev() {
                        ui.horizontal(|ui| {
                            ui.label(&row.file_name);
                            ui.weak(format!("с {}", row.source_host));
                        });
                        ui.horizontal(|ui| match &row.state {
                            RowState::Running => {
                                let fraction = row
                                    .total
                                    .filter(|t| *t > 0)
                                    .map(|t| row.received as f32 / t as f32);
                                let text = match row.total {
                                    Some(total) => {
                                        format!("{} из {}", human(row.received), human(total))
                                    }
                                    None => human(row.received),
                                };
                                match fraction {
                                    Some(f) => ui.add(
                                        egui::ProgressBar::new(f.min(1.0))
                                            .text(text)
                                            .desired_width(320.0),
                                    ),
                                    None => ui.add(
                                        egui::ProgressBar::new(0.0)
                                            .animate(true)
                                            .text(text)
                                            .desired_width(320.0),
                                    ),
                                };
                                if ui.small_button("Отмена").clicked() {
                                    handler.cancel(row.id);
                                }
                            }
                            RowState::Done => {
                                ui.colored_label(
                                    egui::Color32::LIGHT_GREEN,
                                    format!("Готово · {}", human(row.received)),
                                );
                            }
                            RowState::Failed(error) => {
                                ui.colored_label(
                                    egui::Color32::LIGHT_RED,
                                    format!("Ошибка: {error}"),
                                );
                            }
                            RowState::Cancelled => {
                                ui.weak("Отменено");
                            }
                        });
                        ui.add_space(4.0);
                    }
                });
            });
        if !keep_open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        s.parse().unwrap()
    }

    #[test]
    fn archives_and_documents_are_downloads_pages_are_not() {
        assert!(looks_like_download(&u(
            "https://x.test/files/app-1.2.tar.gz"
        )));
        assert!(looks_like_download(&u(
            "https://x.test/a/Report.PDF?token=1"
        )));
        assert!(looks_like_download(&u("http://x.test/setup.exe")));
        assert!(!looks_like_download(&u("https://x.test/")));
        assert!(!looks_like_download(&u("https://x.test/index.html")));
        assert!(!looks_like_download(&u(
            "https://x.test/page.php?file=a.zip"
        )));
        assert!(!looks_like_download(&u("https://x.test/download")));
        assert!(!looks_like_download(&u("file:///tmp/a.zip")));
        assert!(!looks_like_download(&u("data:text/plain,hi")));
    }
}
