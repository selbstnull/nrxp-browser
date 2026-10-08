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
    ConsoleLogLevel, DeviceIntRect, DeviceIntSize, EventLoopWaker, LoadStatus, Preferences,
    RenderingContext, Servo, ServoBuilder, ServoDelegate, SoftwareRenderingContext, WebView,
    WebViewBuilder, WebViewDelegate,
};

/// Настройки движка, не зависящие от конкретной страницы.
#[derive(Debug, Clone, Default)]
pub struct ServoConfig {
    /// `http://127.0.0.1:порт` — прокси для HTTP и HTTPS (CONNECT). Пусто — прямое соединение (I1–I3).
    pub proxy: Option<String>,
    pub user_agent: Option<String>,
    /// Каталог профиля: куки, localStorage, IndexedDB, HSTS, дисковый кэш. `None` — временные данные (пропадают при выходе).
    pub profile_dir: Option<std::path::PathBuf>,
    /// Порт сервера DevTools (слушает только 127.0.0.1). `None` — выключен.
    pub devtools_port: Option<u16>,
    /// JavaScript, внедряемый во все страницы (пользовательские скрипты, отладочные хуки). Применяется при загрузке страницы.
    pub user_scripts: Vec<String>,
}

pub use servo;

mod resources;
pub use resources::ERROR_PAGE_TITLES;

/// Применяет список вида `a,b,-c`: имя включает настройку `dom_<имя>_enabled`, `-имя` выключает. Неизвестные имена — в stderr.
pub fn apply_pref_overrides(prefs: &mut Preferences, spec: &str) {
    for item in spec.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let (on, name) = match item.strip_prefix('-') {
            Some(name) => (false, name),
            None => (true, item),
        };
        let slot: Option<&mut bool> = match name {
            "intersection_observer" => Some(&mut prefs.dom_intersection_observer_enabled),
            "adoptedstylesheet" => Some(&mut prefs.dom_adoptedstylesheet_enabled),
            "fontface" => Some(&mut prefs.dom_fontface_enabled),
            "visual_viewport" => Some(&mut prefs.dom_visual_viewport_enabled),
            "web_animations" => Some(&mut prefs.dom_web_animations_enabled),
            "serviceworker" => Some(&mut prefs.dom_serviceworker_enabled),
            "permissions" => Some(&mut prefs.dom_permissions_enabled),
            "storage_manager" => Some(&mut prefs.dom_storage_manager_api_enabled),
            "exec_command" => Some(&mut prefs.dom_exec_command_enabled),
            "composition_event" => Some(&mut prefs.dom_composition_event_enabled),
            "cookiestore" => Some(&mut prefs.dom_cookiestore_enabled),
            "webgl2" => Some(&mut prefs.dom_webgl2_enabled),
            "offscreen_canvas" => Some(&mut prefs.dom_offscreen_canvas_enabled),
            "async_clipboard" => Some(&mut prefs.dom_async_clipboard_enabled),
            "credential_management" => Some(&mut prefs.dom_credential_management_enabled),
            "worklet" => Some(&mut prefs.dom_worklet_enabled),
            "resize_observer" => Some(&mut prefs.dom_resize_observer_enabled),
            _ => None,
        };
        match slot {
            Some(slot) => *slot = on,
            None => eprintln!("NRB_PREFS: неизвестная настройка «{name}»"),
        }
    }
}

/// Настройки Servo из нашей конфигурации (общие для headless-снимков и оболочки).
pub fn build_preferences(config: &ServoConfig) -> Preferences {
    let mut prefs = Preferences {
        // Самодостаточность: корни webpki зашиты в бинарь, системное хранилище не используется.
        network_use_webpki_roots: true,
        // Экспериментальные в Servo, но нужные реальным сайтам; влияние на корпус отслеживаем (I13).
        dom_indexeddb_enabled: true,
        dom_notification_enabled: true,
        // Без IntersectionObserver не открываются react.dev и svelte.dev (ReferenceError при гидратации): нужен реальным сайтам.
        dom_intersection_observer_enabled: true,
        // adoptedStyleSheets нужны веб-компонентам (GitHub падал с «adoptedStyleSheets is undefined»).
        dom_adoptedstylesheet_enabled: true,
        // События композиции (ввод через IME) — оболочка их шлёт, но без настройки они не доходят до страницы.
        dom_composition_event_enabled: true,
        ..Preferences::default()
    };
    // Экспериментальные переключатели для замеров на корпусе: `NRB_PREFS=intersection_observer,fontface` (включить)
    // и `NRB_PREFS=-fontface` (выключить). Список ограничен известными булевыми настройками `dom_*`.
    if let Ok(spec) = std::env::var("NRB_PREFS") {
        apply_pref_overrides(&mut prefs, &spec);
    }
    if let Some(proxy) = &config.proxy {
        prefs.network_http_proxy_uri = proxy.clone();
        prefs.network_https_proxy_uri = proxy.clone();
    }
    if let Some(ua) = &config.user_agent {
        prefs.user_agent = ua.clone();
    }
    if let Some(port) = config.devtools_port {
        prefs.devtools_server_enabled = true;
        prefs.devtools_server_listen_address = format!("127.0.0.1:{port}");
    }
    if let Some(dir) = &config.profile_dir {
        // Дисковый кэш HTTP рядом с остальными данными профиля.
        prefs.network_http_disk_cache = dir.join("http-cache").to_string_lossy().into_owned();
    }
    prefs
}

/// Опции движка: где хранить данные. Без `profile_dir` хранилище временное.
pub fn build_opts(config: &ServoConfig) -> servo::Opts {
    servo::Opts {
        config_dir: config.profile_dir.clone(),
        temporary_storage: config.profile_dir.is_none(),
        ..servo::Opts::default()
    }
}

/// Установить крипто-провайдер rustls. Servo требует, чтобы процесс сделал это до первой загрузки.
/// Идемпотентно.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[derive(Default)]
struct PageState {
    status: Cell<Option<LoadStatus>>,
    /// Servo сообщил о новом кадре: встраиватель обязан вызвать `WebView::paint` (в окне это делает цикл отрисовки).
    need_paint: Cell<bool>,
    crashed: RefCell<Option<String>>,
    console: RefCell<Vec<String>>,
    screenshot: RefCell<Option<Result<servo::RgbaImage, String>>>,
}

struct PageDelegate(Rc<PageState>);

impl WebViewDelegate for PageDelegate {
    fn show_console_message(&self, _webview: WebView, level: ConsoleLogLevel, message: String) {
        self.0
            .console
            .borrow_mut()
            .push(format!("[{level:?}] {message}"));
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        self.0.status.set(Some(status));
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.0.need_paint.set(true);
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        *self.0.crashed.borrow_mut() = Some(reason);
    }
}

struct ConsoleDelegate(Rc<PageState>);

impl ServoDelegate for ConsoleDelegate {
    fn show_console_message(&self, level: ConsoleLogLevel, message: String) {
        self.0
            .console
            .borrow_mut()
            .push(format!("[{level:?}] {message}"));
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
    user_content: Option<Rc<servo::UserContentManager>>,
}

impl ServoSnapshotter {
    pub fn new(config: &ServoConfig) -> anyhow::Result<Self> {
        install_crypto_provider();

        let servo = ServoBuilder::default()
            .opts(build_opts(config))
            .preferences(build_preferences(config))
            .event_loop_waker(Box::new(PollWaker(Arc::new(AtomicBool::new(false)))))
            .build();
        let state = Rc::new(PageState::default());
        servo.set_delegate(Rc::new(ConsoleDelegate(state.clone())));
        let user_content = (!config.user_scripts.is_empty()).then(|| {
            let manager = Rc::new(servo::UserContentManager::new(&servo));
            for js in &config.user_scripts {
                manager.add_script(Rc::new(servo::UserScript::new(js.clone(), None)));
            }
            manager
        });
        Ok(Self {
            servo,
            state,
            user_content,
        })
    }

    /// Крутит движок, пока `done()` не вернёт `true` или не истечёт срок. `Ok(false)` — срок вышел.
    fn pump(
        &self,
        webview: Option<&WebView>,
        deadline: Instant,
        mut done: impl FnMut() -> bool,
    ) -> Result<bool, EngineError> {
        loop {
            self.servo.spin_event_loop();
            if self.state.need_paint.replace(false)
                && let Some(webview) = webview
            {
                webview.paint();
            }
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
        self.state.need_paint.set(false);
        self.state.console.borrow_mut().clear();
        *self.state.crashed.borrow_mut() = None;
        *self.state.screenshot.borrow_mut() = None;

        let started = Instant::now();
        let deadline = started + req.timeout;
        let mut builder = WebViewBuilder::new(&self.servo, context.clone())
            .url(req.url.clone())
            .delegate(Rc::new(PageDelegate(self.state.clone())));
        if let Some(manager) = &self.user_content {
            builder = builder.user_content_manager(manager.clone());
        }
        let webview = builder.build();

        let state = self.state.clone();
        let completed = self.pump(Some(&webview), deadline, || {
            state.status.get() == Some(LoadStatus::Complete)
        })?;
        let load_time = started.elapsed();

        // Страницы, дорисовывающиеся скриптами: даём движку поработать ещё `settle`.
        if !req.settle.is_zero() {
            self.pump(Some(&webview), Instant::now() + req.settle, || false)?;
        }

        // Штатный скриншот ждёт «полной готовности» (все картинки, шрифты, отсутствие блокирующих
        // ресурсов). У живых сайтов с фоновыми запросами или упавшим скриптом она не наступает —
        // тогда берём то, что уже нарисовано, и помечаем снимок как неполный.
        let mut partial = !completed;
        let mut image = None;
        if completed {
            let sink = self.state.clone();
            webview.take_screenshot(None, move |result| {
                *sink.screenshot.borrow_mut() = Some(result.map_err(|e| format!("{e:?}")));
            });
            let state = self.state.clone();
            let shot_deadline = Instant::now() + req.timeout.min(Duration::from_secs(15));
            if self.pump(Some(&webview), shot_deadline, || {
                state.screenshot.borrow().is_some()
            })? {
                image = self
                    .state
                    .screenshot
                    .borrow_mut()
                    .take()
                    .expect("checked above")
                    .ok();
            }
        }
        let image = match image {
            Some(image) => image,
            None => {
                partial = true;
                webview.paint();
                context
                    .read_to_image(DeviceIntRect::from_size(DeviceIntSize::new(
                        width as i32,
                        height as i32,
                    )))
                    .ok_or_else(|| EngineError::Other("не удалось прочитать кадр".into()))?
            }
        };

        let title = webview.page_title().filter(|t| !t.is_empty());
        let final_url = webview.url();
        let (w, h) = image.dimensions();
        let frame = Frame {
            size: Size {
                width: w,
                height: h,
            },
            rgba: image.into_raw(),
        };
        drop(webview);
        Ok(Snapshot {
            frame,
            load_time,
            console: self.state.console.borrow().clone(),
            partial,
            title,
            final_url,
        })
    }
}
