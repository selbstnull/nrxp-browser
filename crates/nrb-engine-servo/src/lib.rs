//! Реализация [`nrb_engine::Snapshotter`] на Servo (headless, программный рендер).
//!
//! Servo — однопоточный со стороны встраивателя: все вызовы делаются из потока, создавшего `Servo`.
//! Цикл событий здесь — простой опрос `spin_event_loop` (окна и winit нет); интерактивная оболочка
//! (I2) использует тот же `Servo`, но крутит его из winit.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nrb_engine::{EngineError, Frame, Size, Snapshot, SnapshotRequest, Snapshotter};
use servo::{
    ConsoleLogLevel, EventLoopWaker, LoadStatus, Preferences, RenderingContext, Servo,
    ServoBuilder, ServoDelegate, SoftwareRenderingContext, WebView, WebViewBuilder,
    WebViewDelegate,
};

/// Настройки движка, не зависящие от конкретной страницы.
#[derive(Debug, Clone, Default)]
pub struct ServoConfig {
    /// `http://127.0.0.1:порт` — прокси для HTTP и HTTPS (CONNECT). Пусто — прямое соединение (I1–I3).
    pub proxy: Option<String>,
    pub user_agent: Option<String>,
}

/// Установить крипто-провайдер rustls. Servo требует, чтобы процесс сделал это до первой загрузки.
/// Идемпотентно.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[derive(Default)]
struct PageState {
    status: Cell<Option<LoadStatus>>,
    crashed: RefCell<Option<String>>,
    console: RefCell<Vec<String>>,
    screenshot: RefCell<Option<Result<servo::RgbaImage, String>>>,
}

struct PageDelegate(Rc<PageState>);

impl WebViewDelegate for PageDelegate {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        self.0.status.set(Some(status));
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        *self.0.crashed.borrow_mut() = Some(reason);
    }
}

struct ConsoleDelegate(Rc<PageState>);

impl ServoDelegate for ConsoleDelegate {
    fn show_console_message(&self, level: ConsoleLogLevel, message: String) {
        self.0.console.borrow_mut().push(format!("[{level:?}] {message}"));
    }
}

#[derive(Clone)]
struct PollWaker(Arc<AtomicBool>);

impl EventLoopWaker for PollWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

pub struct ServoSnapshotter {
    servo: Servo,
    state: Rc<PageState>,
}

impl ServoSnapshotter {
    pub fn new(config: &ServoConfig) -> anyhow::Result<Self> {
        install_crypto_provider();

        let mut prefs = Preferences::default();
        // Самодостаточность: корни webpki зашиты в бинарь, системное хранилище не используется.
        prefs.network_use_webpki_roots = true;
        if let Some(proxy) = &config.proxy {
            prefs.network_http_proxy_uri = proxy.clone();
            prefs.network_https_proxy_uri = proxy.clone();
        }
        if let Some(ua) = &config.user_agent {
            prefs.user_agent = ua.clone();
        }

        let servo = ServoBuilder::default()
            .preferences(prefs)
            .event_loop_waker(Box::new(PollWaker(Arc::new(AtomicBool::new(false)))))
            .build();
        let state = Rc::new(PageState::default());
        servo.set_delegate(Rc::new(ConsoleDelegate(state.clone())));
        Ok(Self { servo, state })
    }

    /// Крутит движок, пока `done()` не вернёт `true` или не истечёт срок. `Ok(false)` — срок вышел.
    fn pump(&self, deadline: Instant, mut done: impl FnMut() -> bool) -> Result<bool, EngineError> {
        loop {
            self.servo.spin_event_loop();
            if let Some(reason) = self.state.crashed.borrow().clone() {
                return Err(EngineError::Crashed(reason));
            }
            if done() {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Snapshotter for ServoSnapshotter {
    fn snapshot(&mut self, req: &SnapshotRequest) -> Result<Snapshot, EngineError> {
        let Size { width, height } = req.size;
        let context = Rc::new(
            SoftwareRenderingContext::new(dpi::PhysicalSize::new(width, height))
                .map_err(|e| EngineError::Other(format!("RenderingContext: {e:?}")))?,
        );
        context
            .make_current()
            .map_err(|e| EngineError::Other(format!("make_current: {e:?}")))?;

        self.state.status.set(None);
        self.state.console.borrow_mut().clear();
        *self.state.crashed.borrow_mut() = None;
        *self.state.screenshot.borrow_mut() = None;

        let started = Instant::now();
        let deadline = started + req.timeout;
        let webview = WebViewBuilder::new(&self.servo, context.clone())
            .url(req.url.clone())
            .delegate(Rc::new(PageDelegate(self.state.clone())))
            .build();

        let state = self.state.clone();
        let completed = self.pump(deadline, || state.status.get() == Some(LoadStatus::Complete))?;
        if !completed {
            return Err(EngineError::Timeout(req.timeout));
        }
        let load_time = started.elapsed();

        // Страницы, дорисовывающиеся скриптами: даём движку поработать ещё `settle`.
        if !req.settle.is_zero() {
            self.pump(Instant::now() + req.settle, || false)?;
        }

        let sink = self.state.clone();
        webview.take_screenshot(None, move |result| {
            *sink.screenshot.borrow_mut() = Some(result.map_err(|e| format!("{e:?}")));
        });
        let state = self.state.clone();
        let shot_deadline = Instant::now() + req.timeout.min(Duration::from_secs(15));
        if !self.pump(shot_deadline, || state.screenshot.borrow().is_some())? {
            return Err(EngineError::Timeout(req.timeout));
        }
        let image = self
            .state
            .screenshot
            .borrow_mut()
            .take()
            .expect("checked above")
            .map_err(EngineError::Other)?;

        let (w, h) = image.dimensions();
        let frame = Frame { size: Size { width: w, height: h }, rgba: image.into_raw() };
        drop(webview);
        Ok(Snapshot { frame, load_time, console: self.state.console.borrow().clone() })
    }
}
