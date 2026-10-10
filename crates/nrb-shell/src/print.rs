//! Печать в PDF (N6a): растровая. Servo 0.7 не умеет страничную вёрстку, поэтому страница снимается полосами: прокрутка скриптом →
//! пауза на отрисовку → `WebView::take_screenshot` видимой области; полосы склеиваются в длинный снимок, режутся на листы A4 и
//! собираются в PDF (`nrb-pdf`). Ограничения: текст в PDF не выделяется, фиксированные шапки повторяются в каждой полосе, высота
//! ограничена [`MAX_TILES`] экранами.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use nrb_engine_servo::servo::{JSValue, RgbaImage, WebView};

/// Максимум снимков (экранов) на документ: защита от «бесконечных» лент.
pub const MAX_TILES: usize = 40;
/// Пауза после прокрутки перед снимком: Servo должен перерисовать (ленивые картинки, шрифты).
const SETTLE: Duration = Duration::from_millis(450);

const MEASURE_JS: &str = "JSON.stringify({h: Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0), vh: window.innerHeight, vw: window.innerWidth, y: window.scrollY, title: document.title})";

enum Msg {
    Measured(String),
    Scrolled(usize, f64),
    Shot(usize, Result<RgbaImage, String>),
}

enum Stage {
    Measuring,
    Scrolling,
    /// Ждём отрисовку после прокрутки `index`, затем снимаем.
    Settling {
        index: usize,
        until: Instant,
    },
    Capturing,
    Encoding(std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>),
}

pub enum Progress {
    Running(String),
    Done(std::path::PathBuf),
    Failed(String),
}

struct Measure {
    height: f64,
    view_height: f64,
    view_width: f64,
    original_scroll: f64,
    title: String,
}

pub struct PrintJob {
    webview: WebView,
    inbox: Rc<RefCell<Vec<Msg>>>,
    stage: Stage,
    measure: Option<Measure>,
    positions: Vec<f64>,
    actual: Vec<f64>,
    tiles: Vec<(f64, RgbaImage)>,
    truncated: bool,
    out_path: Option<std::path::PathBuf>,
    reserve: Box<dyn Fn(&str) -> Option<std::path::PathBuf>>,
}

fn json_number(json: &str, key: &str) -> Option<f64> {
    let at = json.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = &json[at..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

fn json_string(json: &str, key: &str) -> String {
    let Some(at) = json.find(&format!("\"{key}\":\"")) else {
        return String::new();
    };
    let rest = &json[at + key.len() + 4..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') | Some('t') | Some('r') => out.push(' '),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    out
}

impl PrintJob {
    /// `reserve` — выдаёт свободный путь под PDF по предлагаемому имени (создаёт пустой файл).
    pub fn start(
        webview: WebView,
        reserve: Box<dyn Fn(&str) -> Option<std::path::PathBuf>>,
    ) -> Self {
        let inbox: Rc<RefCell<Vec<Msg>>> = Rc::default();
        let sink = inbox.clone();
        webview.evaluate_javascript(MEASURE_JS, move |result| {
            if let Ok(JSValue::String(json)) = result {
                sink.borrow_mut().push(Msg::Measured(json));
            }
        });
        Self {
            webview,
            inbox,
            stage: Stage::Measuring,
            measure: None,
            positions: Vec::new(),
            actual: Vec::new(),
            tiles: Vec::new(),
            truncated: false,
            out_path: None,
            reserve,
        }
    }

    fn scroll_to(&self, index: usize, y: f64) {
        let sink = self.inbox.clone();
        let script = format!(
            "window.scrollTo({{top: {y}, left: 0, behavior: 'instant'}}); Math.round(window.scrollY * 100) / 100"
        );
        self.webview.evaluate_javascript(script, move |result| {
            let actual = match result {
                Ok(JSValue::Number(n)) => n,
                _ => y,
            };
            sink.borrow_mut().push(Msg::Scrolled(index, actual));
        });
    }

    /// Вызывать из цикла событий; возвращает состояние. После `Done`/`Failed` задачу можно выбросить.
    pub fn poll(&mut self) -> Progress {
        let messages: Vec<Msg> = self.inbox.borrow_mut().drain(..).collect();
        for message in messages {
            match message {
                Msg::Measured(json) => {
                    let (Some(height), Some(view_height), Some(view_width)) = (
                        json_number(&json, "h"),
                        json_number(&json, "vh"),
                        json_number(&json, "vw"),
                    ) else {
                        return Progress::Failed("не удалось измерить страницу".into());
                    };
                    if view_height < 50.0 || view_width < 50.0 || height < 1.0 {
                        return Progress::Failed("страница пустая или слишком маленькая".into());
                    }
                    let max_scroll = (height - view_height).max(0.0);
                    let mut count = (max_scroll / view_height).ceil() as usize + 1;
                    if count > MAX_TILES {
                        count = MAX_TILES;
                        self.truncated = true;
                    }
                    self.positions = (0..count)
                        .map(|i| (i as f64 * view_height).min(max_scroll))
                        .collect();
                    self.measure = Some(Measure {
                        height,
                        view_height,
                        view_width,
                        original_scroll: json_number(&json, "y").unwrap_or(0.0),
                        title: json_string(&json, "title"),
                    });
                    self.stage = Stage::Scrolling;
                    self.scroll_to(0, self.positions[0]);
                }
                Msg::Scrolled(index, actual) => {
                    if matches!(self.stage, Stage::Scrolling) {
                        self.actual.push(actual);
                        self.stage = Stage::Settling {
                            index,
                            until: Instant::now() + SETTLE,
                        };
                    }
                }
                Msg::Shot(index, result) => match result {
                    Ok(image) => {
                        let y = self.actual.get(index).copied().unwrap_or(0.0);
                        self.tiles.push((y, image));
                        if index + 1 < self.positions.len() {
                            self.stage = Stage::Scrolling;
                            self.scroll_to(index + 1, self.positions[index + 1]);
                        } else {
                            return self.finish();
                        }
                    }
                    Err(error) => return Progress::Failed(format!("снимок: {error}")),
                },
            }
        }
        match &self.stage {
            Stage::Settling { index, until } if Instant::now() >= *until => {
                let index = *index;
                self.stage = Stage::Capturing;
                let sink = self.inbox.clone();
                self.webview.take_screenshot(None, move |result| {
                    sink.borrow_mut()
                        .push(Msg::Shot(index, result.map_err(|e| format!("{e:?}"))));
                });
            }
            Stage::Encoding(receiver) => match receiver.try_recv() {
                Ok(Ok(path)) => return Progress::Done(path),
                Ok(Err(error)) => return Progress::Failed(error),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Progress::Failed("поток кодирования завершился".into());
                }
            },
            _ => {}
        }
        Progress::Running(match &self.stage {
            Stage::Measuring => "Печать: измеряю страницу…".into(),
            Stage::Encoding(_) => "Печать: собираю PDF…".into(),
            _ => format!(
                "Печать: снимок {} из {}…",
                self.tiles.len() + 1,
                self.positions.len().max(1)
            ),
        })
    }

    fn finish(&mut self) -> Progress {
        let Some(measure) = self.measure.as_ref() else {
            return Progress::Failed("нет измерений".into());
        };
        // Вернуть прокрутку, как была.
        self.webview.evaluate_javascript(
            format!(
                "window.scrollTo({{top: {}, left: 0, behavior: 'instant'}}); 0",
                measure.original_scroll
            ),
            |_| {},
        );
        let Some(first) = self.tiles.first() else {
            return Progress::Failed("нет снимков".into());
        };
        let image_width = first.1.width();
        let scale = f64::from(image_width) / measure.view_width;
        let total_height = (measure
            .height
            .min(self.positions.last().copied().unwrap_or(0.0) + measure.view_height)
            * scale)
            .round() as u32;
        let tiles: Vec<(u32, RgbaImage)> = std::mem::take(&mut self.tiles)
            .into_iter()
            .map(|(y, image)| ((y * scale).round() as u32, image))
            .collect();
        let name = if measure.title.trim().is_empty() {
            "страница.pdf".to_owned()
        } else {
            format!("{}.pdf", measure.title.trim())
        };
        let Some(path) = (self.reserve)(&name) else {
            return Progress::Failed("не удалось выбрать файл в каталоге загрузок".into());
        };
        self.out_path = Some(path.clone());
        let title = measure.title.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let full = nrb_pdf::stitch(&tiles, image_width, total_height);
                let bytes =
                    nrb_pdf::pdf_from_image(&full, &title, 85).map_err(|e| e.to_string())?;
                std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(path)
            })();
            let _ = sender.send(result);
        });
        self.stage = Stage::Encoding(receiver);
        Progress::Running("Печать: собираю PDF…".into())
    }

    /// Обрезан ли документ по лимиту экранов.
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_json_is_parsed() {
        let json = "{\"h\":3200,\"vh\":800,\"vw\":1280,\"y\":12.5,\"title\":\"Привет \\\"мир\\\"\\u0021\"}";
        assert_eq!(json_number(json, "h"), Some(3200.0));
        assert_eq!(json_number(json, "y"), Some(12.5));
        assert_eq!(json_string(json, "title"), "Привет \"мир\"!");
        assert_eq!(json_number(json, "нет"), None);
    }
}
