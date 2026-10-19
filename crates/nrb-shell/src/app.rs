use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use nrb_engine::omnibox;
use nrb_engine_servo::servo::{
    AllowOrDenyRequest, AuthenticationRequest, CompositionEvent, CompositionState,
    CreateNewWebViewRequest, Cursor, DeviceIntRect, DevicePoint, DownloadEvent, DownloadRequest,
    DownloadStream, EmbedderControl, EmbedderControlId, EventLoopWaker, ImeEvent, InputEvent,
    KeyboardEvent as ServoKeyboardEvent, LoadStatus, MouseButton, MouseButtonAction,
    MouseButtonEvent, MouseMoveEvent, OffscreenRenderingContext, PermissionRequest, PixelFormat,
    RenderingContext, Servo, ServoBuilder, ServoDelegate, SimpleDialog, TouchEvent, TouchEventType,
    TouchId, TouchPointerType, WebResourceLoad, WebResourceResponse, WebView, WebViewBuilder,
    WebViewDelegate, WebViewId, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
    WindowRenderingContext,
};
use nrb_engine_servo::servo::{UserContentManager, UserScript};
use nrb_engine_servo::{
    ServoConfig, build_opts, build_preferences, install_crypto_provider, install_webrtc_policy,
};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton as WMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey as WNamedKey};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{CursorIcon, Window, WindowId};

use crate::downloads::{
    BodyEvent, DownloadHandler, DownloadsWindow, IncomingDownload, looks_like_download,
};
use crate::find::{self, FindAction, FindBar};
use crate::keys;
use crate::library::{Library, LibraryAction, Panel};
use crate::modal::{self, Modal, feature_key};
use crate::nodes::{NodeManager, NodesWindow};
use crate::passwords::{self, Effect as PwEffect, Passwords};
use crate::print::{PrintJob, Progress};
use crate::site_data::SiteDataWindow;
use crate::ui::{self, Action, TabAudioState, TabInfo, ToolbarState};
use nrb_engine_servo::servo::{FindAction as EngineFindAction, FindRequest};

/// Состояние туннеля для индикатора в тулбаре. Реализует вызывающий код (`nrb`), оболочка о NRXP ничего не знает.
pub trait TunnelStatus: Send + Sync {
    fn up(&self) -> bool;
    /// Короткая строка для бейджа, например «NRXP host:port · ног 4 · ↓1.2 МБ».
    fn summary(&self) -> String;
}

/// Настройки оболочки.
#[derive(Clone)]
pub struct ShellConfig {
    pub initial_urls: Vec<Url>,
    pub servo: ServoConfig,
    pub search: String,
    pub tunnel: Option<std::sync::Arc<dyn TunnelStatus>>,
    /// Менеджер узлов для окна «Узлы» (Ctrl+Shift+N); без него пункт меню не работает.
    pub nodes: Option<std::sync::Arc<dyn NodeManager>>,
    /// Обработчик загрузок файлов (Ctrl+J); без него ссылки на файлы открываются как обычно.
    pub downloads: Option<std::sync::Arc<dyn DownloadHandler>>,
    /// Блокировщик рекламы; без него (или без правил) запросы не фильтруются.
    pub adblock: Option<std::sync::Arc<nrb_adblock::Blocker>>,
    /// Готовый JavaScript пользовательских скриптов/стилей (`nrb_userscripts::UserContent::wrap`); внедряется во все вкладки.
    pub userscripts: Vec<String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            initial_urls: Vec::new(),
            servo: ServoConfig::default(),
            search: omnibox::DEFAULT_SEARCH.into(),
            tunnel: None,
            nodes: None,
            downloads: None,
            adblock: None,
            userscripts: Vec::new(),
        }
    }
}

pub fn run(config: ShellConfig) -> anyhow::Result<()> {
    let event_loop = EventLoop::<WakerEvent>::with_user_event().build()?;
    run_event_loop(event_loop, config)
}

/// Точка входа Android: цикл событий строится поверх `AndroidApp` (его отдаёт `android_main` в крейте nrb-android).
#[cfg(target_os = "android")]
pub fn run_android(
    config: ShellConfig,
    app: winit::platform::android::activity::AndroidApp,
) -> anyhow::Result<()> {
    use winit::platform::android::EventLoopBuilderExtAndroid;
    let event_loop = EventLoop::<WakerEvent>::with_user_event()
        .with_android_app(app)
        .build()?;
    run_event_loop(event_loop, config)
}

fn run_event_loop(event_loop: EventLoop<WakerEvent>, config: ShellConfig) -> anyhow::Result<()> {
    install_crypto_provider();
    let mut app = App::Initial {
        proxy: event_loop.create_proxy(),
        config: Some(Box::new(config)),
    };
    let result = event_loop.run_app(&mut app);
    // Не разбираем Servo при выходе: его остановка может упасть с abort (assert в WebGL, `is_deleted`) уже после того, как
    // сессия и профиль сохранены, и это превращает обычное закрытие окна в аварийное. Процесс всё равно завершается.
    std::mem::forget(app);
    result?;
    Ok(())
}

#[derive(Debug)]
struct WakerEvent;

#[derive(Clone)]
struct Waker(EventLoopProxy<WakerEvent>);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        let _ = self.0.send_event(WakerEvent);
    }
}

/// То, что делегат Servo кладёт для оболочки (Servo зовёт делегата из нашего же потока во время `spin_event_loop`).
struct Shared {
    /// Кто сколько раз просил перерисовку за интервал диагностики (см. `Shared::redraw_why`).
    redraw_reasons: RefCell<std::collections::HashMap<String, u32>>,
    window: RefCell<Option<Rc<Window>>>,
    content: RefCell<Option<Rc<OffscreenRenderingContext>>>,
    scale: Cell<f32>,
    toolbar_px: Cell<u32>,
    new_tabs: RefCell<Vec<WebView>>,
    closed: RefCell<Vec<WebView>>,
    /// Новые модальные запросы от Servo (id элемента управления нужен, чтобы убрать окно по `hide_embedder_control`).
    modals: RefCell<Vec<(Option<EmbedderControlId>, Modal)>>,
    hidden_controls: RefCell<Vec<EmbedderControlId>>,
    fullscreen: Cell<Option<bool>>,
    favicons: RefCell<Vec<WebViewId>>,
    /// Страницы, дозагрузившиеся до `Complete` (для журнала), и сменившиеся заголовки.
    completed: RefCell<Vec<WebViewId>>,
    titled: RefCell<Vec<WebViewId>>,
    /// Блокировщик рекламы (если есть списки) и его счётчик; включается настройкой `adblock`.
    blocker: Option<std::sync::Arc<nrb_adblock::Blocker>>,
    adblock_on: Cell<bool>,
    blocked: Cell<u64>,
    /// Включён ли перехват загрузок (есть обработчик) и очередь перехваченных: (адрес, страница-источник).
    downloads_enabled: Cell<bool>,
    download_requests: RefCell<Vec<(Url, Option<Url>)>>,
    /// Загрузки, принятые у движка (N3b): описание и поток тела; передаются обработчику из главного цикла.
    engine_downloads: RefCell<Vec<(IncomingDownload, DownloadStream)>>,
    /// Запомненные решения о разрешениях: (источник, ключ возможности) → разрешено. Копия таблицы профиля для делегата.
    permissions: RefCell<std::collections::HashMap<(String, String), bool>>,
    /// Вкладки, где страница дошла до `HeadParsed`/`Complete`: туда нужно (пере)применить звуковую политику.
    audio_apply: RefCell<Vec<WebViewId>>,
    /// Ответы опроса звука: (вкладка, звучит ли).
    audio_inbox: Rc<RefCell<Vec<(WebViewId, bool)>>>,
}

fn load_permissions(
    store: &nrb_profile::Store,
) -> std::collections::HashMap<(String, String), bool> {
    store
        .permissions()
        .unwrap_or_default()
        .into_iter()
        .map(|p| ((p.origin, p.feature), p.allow))
        .collect()
}

/// Встроенный скрипт звука (во всех фреймах): политика автовоспроизведения, заглушение вкладки, состояние для индикатора.
const AUDIO_JS: &str = include_str!("audio.js");

/// Встроенный скрипт менеджера паролей (во всех фреймах): ловит введённый вход, подставляет по команде.
const PWM_JS: &str = include_str!("pwm.js");

/// Настройки движка, зависящие от настроек профиля, меняются на лету (читаются при создании страницы/на каждый запрос):
/// сторонние куки и WebRTC (интерфейс `RTCPeerConnection` появляется у новых страниц; запрещён в герметичном режиме).
fn apply_engine_policy(settings: &nrb_profile::Settings, webrtc_forbidden: bool) {
    use nrb_engine_servo::servo::prefs;
    let webrtc = settings.webrtc
        && !webrtc_forbidden
        // Политика WebRTC (relay-only через туннель) — статик процесса и в контент-процессы не попадает.
        && !nrb_engine_servo::multiprocess_enabled();
    let mut preferences = prefs::get().clone();
    if preferences.network_block_third_party_cookies != settings.block_third_party_cookies
        || preferences.dom_webrtc_enabled != webrtc
    {
        preferences.network_block_third_party_cookies = settings.block_third_party_cookies;
        preferences.dom_webrtc_enabled = webrtc;
        preferences.dom_webrtc_transceiver_enabled = webrtc;
        prefs::set(preferences);
    }
}

/// Адрес источника для подписи окон: `https://host`; для страниц без хоста — полный URL.
fn origin_of(webview: &WebView) -> String {
    match webview.url() {
        Some(url) => match url.host_str() {
            Some(host) => format!("{}://{host}", url.scheme()),
            None => url.to_string(),
        },
        None => "(неизвестная страница)".into(),
    }
}

impl Shared {
    fn redraw(&self) {
        self.redraw_why("redraw");
    }

    /// Просит перерисовку; `why` — кто просил (счётчик в диагностике простоя `RUST_LOG=nrb_shell=debug`).
    fn redraw_why(&self, why: &'static str) {
        *self.redraw_reasons.borrow_mut().entry(why.to_owned()).or_insert(0) += 1;
        if let Some(window) = self.window.borrow().as_ref() {
            window.request_redraw();
        }
    }
}

/// Время одной фазы кадра за интервал диагностики: среднее и максимум, мс.
#[derive(Default)]
struct PhaseTime {
    sum: Duration,
    max: Duration,
    n: u32,
}

impl PhaseTime {
    fn add(&mut self, d: Duration) {
        self.sum += d;
        self.max = self.max.max(d);
        self.n += 1;
    }

    fn take(&mut self) -> String {
        let t = std::mem::take(self);
        format!(
            "{:.1}/макс {:.1}",
            t.sum.as_secs_f64() * 1000.0 / f64::from(t.n.max(1)),
            t.max.as_secs_f64() * 1000.0
        )
    }
}

struct FrameStats {
    page: PhaseTime,
    egui: PhaseTime,
    present: PhaseTime,
    gap_max: Duration,
    last_end: Instant,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            page: PhaseTime::default(),
            egui: PhaseTime::default(),
            present: PhaseTime::default(),
            gap_max: Duration::ZERO,
            last_end: Instant::now(),
        }
    }
}

struct Delegate(Rc<Shared>);

impl WebViewDelegate for Delegate {
    /// Размер экрана и окна для `screen.width`, `screen.availWidth`, `window.outerWidth`…: без этого страница видит экран 0×0,
    /// а YouTube по такому «экрану» не предлагает качество выше 1080p.
    fn screen_geometry(&self, _webview: WebView) -> Option<nrb_engine_servo::servo::ScreenGeometry> {
        use nrb_engine_servo::servo::{DeviceIntPoint, DeviceIntRect, DeviceIntSize, ScreenGeometry};
        let window = self.0.window.borrow().clone()?;
        let monitor = window.current_monitor().or_else(|| window.primary_monitor())?;
        let screen = monitor.size();
        let position = window.outer_position().unwrap_or_default();
        let outer = window.outer_size();
        let screen = DeviceIntSize::new(screen.width as i32, screen.height as i32);
        Some(ScreenGeometry {
            size: screen,
            // Панель задач неизвестна — считаем весь экран доступным.
            available_size: screen,
            window_rect: DeviceIntRect::from_origin_and_size(
                DeviceIntPoint::new(position.x, position.y),
                DeviceIntSize::new(outer.width as i32, outer.height as i32),
            ),
        })
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.0.redraw_why("notify_new_frame_ready");
    }

    fn notify_url_changed(&self, _webview: WebView, _url: Url) {
        self.0.redraw_why("notify_url_changed");
    }

    fn notify_page_title_changed(&self, webview: WebView, _title: Option<String>) {
        self.0.titled.borrow_mut().push(webview.id());
        self.0.redraw_why("notify_page_title_changed");
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        if status == LoadStatus::Complete {
            self.0.completed.borrow_mut().push(webview.id());
        }
        if matches!(status, LoadStatus::HeadParsed | LoadStatus::Complete) {
            self.0.audio_apply.borrow_mut().push(webview.id());
        }
        self.0.redraw_why("notify_load_status_changed");
    }

    /// Движок сообщил, что ответ навигации — файл (N3b): принимаем и передаём тело обработчику загрузок. Без обработчика
    /// (приватный режим без каталога и т. п.) отказываем — движок поведёт себя как раньше.
    fn request_download(&self, _webview: WebView, request: DownloadRequest) {
        if !self.0.downloads_enabled.get() {
            request.decline();
            return;
        }
        let incoming = IncomingDownload {
            url: request.url(),
            suggested_name: request.suggested_filename().map(str::to_owned),
            mime: request.mime().map(str::to_owned),
            total: request.content_length(),
        };
        // Небольшой буфер: медленный диск замедляет сеть, а не раздувает память.
        let stream = request.accept(16);
        self.0
            .engine_downloads
            .borrow_mut()
            .push((incoming, stream));
        self.0.redraw_why("request_download");
    }

    /// Блокировка рекламы и трекеров до сети. Главный документ не трогаем; страница верхнего уровня — источник для `$third-party`.
    fn load_web_resource(&self, webview: WebView, load: WebResourceLoad) {
        // Загрузки файлов (N3a): главный GET-запрос на «файловый» адрес не показываем, а отдаём загрузчику. Ответ 204 оставляет
        // текущую страницу на месте (как у настоящих браузеров). Настоящее решение — в движке (nxrp-servo N3b).
        if self.0.downloads_enabled.get() {
            let request = load.request();
            if request.is_for_main_frame
                && request.method == http::Method::GET
                && looks_like_download(&request.url)
            {
                let url = request.url.clone();
                self.0
                    .download_requests
                    .borrow_mut()
                    .push((url.clone(), webview.url()));
                load.intercept(
                    WebResourceResponse::new(url).status_code(http::StatusCode::NO_CONTENT),
                )
                .finish();
                self.0.redraw_why("load_web_resource");
                return;
            }
        }
        let Some(blocker) = self.0.blocker.as_ref() else {
            return;
        };
        if !self.0.adblock_on.get() || load.request().is_for_main_frame {
            return;
        }
        let url = load.request().url.clone();
        let kind = nrb_adblock::ResourceKind::from_destination(&format!(
            "{:?}",
            load.request().destination
        ));
        let source = webview.url().map(|u| u.to_string()).unwrap_or_default();
        if blocker.check(url.as_str(), &source, kind) == nrb_adblock::Verdict::Block {
            self.0.blocked.set(self.0.blocked.get() + 1);
            load.intercept(WebResourceResponse::new(url)).cancel();
            self.0.redraw_why("load_web_resource");
        }
    }

    fn notify_history_changed(&self, _webview: WebView, _entries: Vec<Url>, _current: usize) {
        self.0.redraw_why("notify_history_changed");
    }

    fn notify_cursor_changed(&self, _webview: WebView, _cursor: Cursor) {
        self.0.redraw_why("notify_cursor_changed");
    }

    fn notify_favicon_changed(&self, webview: WebView) {
        self.0.favicons.borrow_mut().push(webview.id());
        self.0.redraw_why("notify_favicon_changed");
    }

    fn notify_fullscreen_state_changed(&self, _webview: WebView, fullscreen: bool) {
        self.0.fullscreen.set(Some(fullscreen));
        self.0.redraw_why("notify_fullscreen_state_changed");
    }

    fn request_permission(&self, webview: WebView, request: PermissionRequest) {
        let origin = origin_of(&webview);
        let feature = request.feature();
        // Ключ запоминания — полный источник с портом (`localhost:3000` и `localhost:8080` — разные сайты).
        let key = webview
            .url()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
            .map(|url| {
                (
                    url.origin().ascii_serialization(),
                    feature_key(feature).to_owned(),
                )
            });
        if let Some(key) = &key
            && let Some(&allow) = self.0.permissions.borrow().get(key)
        {
            if allow {
                request.allow();
            } else {
                request.deny();
            }
            return;
        }
        self.0.modals.borrow_mut().push((
            None,
            Modal::Permission {
                origin,
                feature,
                request: Some(request),
                key,
                remember: false,
                decision: None,
            },
        ));
        self.0.redraw_why("request_permission");
    }

    fn request_authentication(&self, webview: WebView, request: AuthenticationRequest) {
        let _ = &webview;
        let (url, for_proxy) = (request.url().to_string(), request.for_proxy());
        self.0.modals.borrow_mut().push((
            None,
            Modal::Auth {
                url,
                for_proxy,
                request: Some(request),
                user: String::new(),
                pass: String::new(),
            },
        ));
        self.0.redraw_why("request_authentication");
    }

    fn show_embedder_control(&self, webview: WebView, control: EmbedderControl) {
        let id = control.id();
        let scale = self.0.scale.get().max(0.5);
        let toolbar = self.0.toolbar_px.get() as f32;
        let at = |rect: DeviceIntRect| {
            egui::pos2(
                rect.min.x as f32 / scale,
                (rect.max.y as f32 + toolbar) / scale,
            )
        };
        let modal = match control {
            EmbedderControl::SimpleDialog(dialog) => {
                let input = match &dialog {
                    SimpleDialog::Prompt(prompt) => prompt.current_value().to_owned(),
                    _ => String::new(),
                };
                Modal::Dialog {
                    origin: origin_of(&webview),
                    dialog: Some(dialog),
                    input,
                }
            }
            EmbedderControl::SelectElement(element) => Modal::Select {
                anchor: at(element.position()),
                selected: element.selected_options(),
                element: Some(element),
            },
            EmbedderControl::ContextMenu(menu) => Modal::Context {
                anchor: at(menu.position()),
                menu: Some(menu),
            },
            // Выбор файла/цвета и ввод через IME-панель пока не поддерживаются: запрос отменяется при drop.
            _ => return,
        };
        self.0.modals.borrow_mut().push((Some(id), modal));
        self.0.redraw_why("show_embedder_control");
    }

    fn hide_embedder_control(&self, _webview: WebView, id: EmbedderControlId) {
        self.0.hidden_controls.borrow_mut().push(id);
        self.0.redraw_why("hide_embedder_control");
    }

    fn notify_closed(&self, webview: WebView) {
        self.0.closed.borrow_mut().push(webview);
        self.0.redraw_why("notify_closed");
    }

    fn notify_crashed(&self, webview: WebView, reason: String, _backtrace: Option<String>) {
        tracing::error!(%reason, "вкладка упала");
        let page = format!(
            "data:text/html;charset=utf-8,<h2>Вкладка упала</h2><pre>{}</pre>",
            reason.replace('<', "&lt;")
        );
        if let Ok(url) = Url::parse(&page) {
            webview.load(url);
        }
    }

    fn request_create_new(&self, parent: WebView, request: CreateNewWebViewRequest) {
        let Some(content) = self.0.content.borrow().clone() else {
            return;
        };
        let webview = request
            .builder(content)
            .delegate(parent.delegate())
            .hidpi_scale_factor(euclid_compat::scale(self.0.scale.get()))
            .build();
        self.0.new_tabs.borrow_mut().push(webview);
        self.0.redraw_why("request_create_new");
    }
}

/// Делегат самого Servo: сервер DevTools (включается флагом `--devtools`, слушает только loopback).
struct EngineDelegate;

impl ServoDelegate for EngineDelegate {
    fn notify_devtools_server_started(&self, port: u16, token: String) {
        eprintln!("DevTools: 127.0.0.1:{port} (токен для подключения без запроса: {token})");
    }

    fn request_devtools_connection(&self, request: AllowOrDenyRequest) {
        // Сервер включён пользователем явно и доступен только с этого компьютера.
        request.allow();
    }
}

enum App {
    Initial {
        proxy: EventLoopProxy<WakerEvent>,
        config: Option<Box<ShellConfig>>,
    },
    Running(Box<Running>),
}

struct Running {
    window: Rc<Window>,
    servo: Servo,
    user_content: Option<Rc<UserContentManager>>,
    window_ctx: Rc<WindowRenderingContext>,
    content_ctx: Rc<OffscreenRenderingContext>,
    gl: Arc<egui_glow::glow::Context>,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    painter: egui_glow::Painter,
    shared: Rc<Shared>,
    delegate: Rc<Delegate>,
    tabs: Vec<WebView>,
    active: usize,
    toolbar: ToolbarState,
    toolbar_px: u32,
    cursor: PhysicalPosition<f64>,
    modifiers: ModifiersState,
    search: String,
    exit: bool,
    tunnel: Option<std::sync::Arc<dyn TunnelStatus>>,
    nodes: Option<std::sync::Arc<dyn NodeManager>>,
    nodes_window: NodesWindow,
    downloads: Option<std::sync::Arc<dyn DownloadHandler>>,
    downloads_window: DownloadsWindow,
    find_bar: FindBar,
    site_data_window: SiteDataWindow,
    print_job: Option<PrintJob>,
    print_notice: Option<(String, Instant)>,
    next_badge_refresh: std::time::Instant,
    /// Перерисовку кто-то запросил (иначе `RedrawRequested` пришёл от ОС: WM_PAINT/обновление окна).
    redraw_requested: std::cell::Cell<bool>,
    /// Накопленная, ещё не отданная движку прокрутка колесом (пиксели), и время последней порции.
    wheel_pending: (f64, f64),
    wheel_tick: Instant,
    frame_stats: FrameStats,
    frames: u32,
    frames_repaint_egui: u32,
    frames_page_animating: u32,
    frames_since: Instant,
    /// Звук вкладок: опрос раз в 1,5 с; заглушённые вкладки и последнее известное «звучит».
    next_audio_poll: std::time::Instant,
    webrtc_forbidden: bool,
    /// Android: поверхность окна исчезла (приложение свёрнуто) — не рисуем, пока она не вернётся.
    suspended: bool,
    passwords: Passwords,
    muted_tabs: std::collections::HashSet<WebViewId>,
    audible_tabs: std::collections::HashSet<WebViewId>,
    /// Очередь модальных запросов Servo; рисуется первое, остальные ждут.
    modals: std::collections::VecDeque<(Option<EmbedderControlId>, Modal)>,
    /// Окно в полноэкранном режиме (F11 или запрос страницы): тулбар скрыт.
    fullscreen: bool,
    favicons: std::collections::HashMap<WebViewId, egui::TextureHandle>,
    store: nrb_profile::Store,
    settings: nrb_profile::Settings,
    profile_dir: Option<std::path::PathBuf>,
    library: Library,
    /// Последняя сохранённая сессия и время сохранения (дебаунс).
    saved_session: (nrb_profile::SessionState, std::time::Instant),
    /// Кэш «текущая страница в закладках» по адресу.
    bookmark_cache: (String, bool),
}

impl ApplicationHandler<WakerEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let App::Running(running) = self {
            // Android: поверхность вернулась. Контекст Servo привязан к старой поверхности — пересоздание не сделано, пока
            // страница не перерисуется после смены размера (см. docs/PLATFORMS.md, «Жизненный цикл»).
            if running.suspended {
                running.suspended = false;
                let size = running.window.inner_size();
                running.window_ctx.resize(size);
                running.window.request_redraw();
            }
            return;
        }
        let App::Initial { proxy, config } = self else {
            return;
        };
        let config = config.take().map(|c| *c).unwrap_or_default();
        match Running::new(event_loop, proxy.clone(), config) {
            Ok(running) => *self = App::Running(Box::new(running)),
            Err(error) => {
                eprintln!("не удалось запустить оболочку: {error:#}");
                event_loop.exit();
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let App::Running(running) = self {
            running.suspended = true;
            running.save_session(true);
        }
    }

    /// Бейдж туннеля меняется без событий окна — раз в секунду просим перерисовку.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Печать идёт по таймеру (пауза на отрисовку между полосами) — крутим цикл, пока задача жива.
        if let App::Running(running) = self
            && running.print_job.is_some()
        {
            running.spin();
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                std::time::Instant::now() + std::time::Duration::from_millis(40),
            ));
            return;
        }
        if let App::Running(running) = self {
            let now = std::time::Instant::now();
            if running.tunnel.is_some() && now >= running.next_badge_refresh {
                running.next_badge_refresh = now + std::time::Duration::from_secs(1);
                running.window.request_redraw();
            }
            // Звук вкладок опрашивается по таймеру: будим цикл к ближайшему сроку.
            running.poll_audio();
            let wake = if running.tunnel.is_some() {
                running.next_badge_refresh.min(running.next_audio_poll)
            } else {
                running.next_audio_poll
            };
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(wake));
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: WakerEvent) {
        if let App::Running(running) = self {
            running.spin();
            if running.exit {
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let App::Running(running) = self {
            running.on_window_event(event_loop, event);
            if running.exit {
                event_loop.exit();
            }
        }
    }
}

mod euclid_compat {
    use euclid::Scale;

    pub fn scale<A, B>(factor: f32) -> Scale<f32, A, B> {
        Scale::new(factor)
    }
}

impl Running {
    fn new(
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<WakerEvent>,
        config: ShellConfig,
    ) -> anyhow::Result<Self> {
        let window = Rc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("nrb")
                    .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 860.0)),
            )?,
        );
        let display_handle = event_loop.display_handle()?;
        let window_handle = window.window_handle()?;
        let size = window.inner_size();

        let window_ctx = Rc::new(
            WindowRenderingContext::new(display_handle, window_handle, size)
                .map_err(|e| anyhow::anyhow!("WindowRenderingContext: {e:?}"))?,
        );
        window_ctx
            .make_current()
            .map_err(|e| anyhow::anyhow!("make_current: {e:?}"))?;
        let gl = window_ctx.glow_gl_api();

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let painter = egui_glow::Painter::new(gl.clone(), "", None, false)
            .map_err(|e| anyhow::anyhow!("egui painter: {e}"))?;

        let scale = window.scale_factor() as f32;
        let toolbar_px = (68.0 * scale).round() as u32;
        let content_size = PhysicalSize::new(
            size.width.max(1),
            size.height.saturating_sub(toolbar_px).max(1),
        );
        let content_ctx = Rc::new(window_ctx.offscreen_context(content_size));

        // Политика WebRTC (relay через прокси туннеля) должна стоять до создания первого соединения.
        install_webrtc_policy(&config.servo);
        let servo = nrb_engine_servo::apply_network(
            ServoBuilder::default()
                .opts(build_opts(&config.servo))
                .preferences(build_preferences(&config.servo))
                .event_loop_waker(Box::new(Waker(proxy))),
            &config.servo,
        )
        .build();
        if config.servo.devtools_port.is_some() {
            servo.set_delegate(Rc::new(EngineDelegate));
        }
        // Пользовательские скрипты применяются при загрузке страницы (менеджер общий для всех вкладок).
        let user_content = Some({
            let manager = Rc::new(UserContentManager::new(&servo));
            // Звуковая политика (автовоспроизведение, заглушение вкладки) — всегда, до пользовательских скриптов.
            manager.add_script(Rc::new(UserScript::new(
                nrb_engine_servo::COMPAT_JS.to_owned(),
                None,
            )));
            manager.add_script(Rc::new(UserScript::new(AUDIO_JS.to_owned(), None)));
            manager.add_script(Rc::new(UserScript::new(PWM_JS.to_owned(), None)));
            for js in &config.userscripts {
                manager.add_script(Rc::new(UserScript::new(js.clone(), None)));
            }
            manager
        });

        let shared = Rc::new(Shared {
            redraw_reasons: RefCell::default(),
            window: RefCell::new(Some(window.clone())),
            content: RefCell::new(Some(content_ctx.clone())),
            scale: Cell::new(scale),
            toolbar_px: Cell::new(toolbar_px),
            new_tabs: RefCell::default(),
            closed: RefCell::default(),
            modals: RefCell::default(),
            hidden_controls: RefCell::default(),
            fullscreen: Cell::new(None),
            favicons: RefCell::default(),
            blocker: config.adblock.filter(|b| b.rule_count() > 0),
            adblock_on: Cell::new(false),
            blocked: Cell::new(0),
            downloads_enabled: Cell::new(config.downloads.is_some()),
            download_requests: RefCell::default(),
            engine_downloads: RefCell::default(),
            permissions: RefCell::default(),
            audio_apply: RefCell::default(),
            audio_inbox: Rc::default(),
            completed: RefCell::default(),
            titled: RefCell::default(),
        });
        let delegate = Rc::new(Delegate(shared.clone()));

        let profile_dir = config.servo.profile_dir.clone();
        let (store, settings) = match &profile_dir {
            Some(dir) => {
                let store = nrb_profile::Store::open(dir).or_else(|error| {
                    tracing::warn!(%error, "не удалось открыть базу профиля — работаем без сохранения");
                    nrb_profile::Store::in_memory()
                })?;
                let settings = nrb_profile::Settings::load(dir).unwrap_or_else(|error| {
                    tracing::warn!(%error, "настройки не прочитаны — значения по умолчанию");
                    nrb_profile::Settings::default()
                });
                (store, settings)
            }
            None => (
                nrb_profile::Store::in_memory()?,
                nrb_profile::Settings::default(),
            ),
        };
        shared.adblock_on.set(settings.adblock);
        apply_engine_policy(&settings, config.servo.webrtc_forbidden);
        *shared.permissions.borrow_mut() = load_permissions(&store);
        crate::theme::apply(&egui_ctx, settings.theme);

        let mut running = Self {
            window,
            servo,
            user_content,
            window_ctx,
            content_ctx,
            gl,
            egui_ctx,
            egui_state,
            painter,
            shared,
            delegate,
            tabs: Vec::new(),
            active: 0,
            toolbar: ToolbarState {
                url_input: String::new(),
                focus_url: false,
                can_back: false,
                can_forward: false,
                loading: false,
                tunnel: None,
                blocked: None,
                notice: None,
                bookmarked: false,
                login_form: false,
                suggestions: Vec::new(),
                suggest_open: false,
                input_changed: false,
            },
            toolbar_px,
            cursor: PhysicalPosition::new(0.0, 0.0),
            modifiers: ModifiersState::empty(),
            search: settings.search.clone(),
            exit: false,
            tunnel: config.tunnel,
            next_badge_refresh: std::time::Instant::now(),
            redraw_requested: std::cell::Cell::new(false),
            wheel_pending: (0.0, 0.0),
            wheel_tick: Instant::now(),
            frame_stats: FrameStats::default(),
            frames: 0,
            frames_repaint_egui: 0,
            frames_page_animating: 0,
            frames_since: Instant::now(),
            next_audio_poll: std::time::Instant::now(),
            webrtc_forbidden: config.servo.webrtc_forbidden,
            suspended: false,
            passwords: Passwords::new(config.servo.profile_dir.as_deref(), settings.passwords),
            muted_tabs: Default::default(),
            audible_tabs: Default::default(),
            modals: Default::default(),
            fullscreen: false,
            favicons: Default::default(),
            store,
            settings,
            profile_dir,
            library: Library::default(),
            nodes: config.nodes,
            nodes_window: NodesWindow::default(),
            downloads: config.downloads,
            downloads_window: DownloadsWindow::default(),
            find_bar: FindBar::default(),
            site_data_window: SiteDataWindow::default(),
            print_job: None,
            print_notice: None,
            saved_session: (
                nrb_profile::SessionState::default(),
                std::time::Instant::now(),
            ),
            bookmark_cache: (String::new(), false),
        };
        running.window.set_ime_allowed(true);

        let _ = &config.search; // шаблон поиска теперь из настроек профиля
        if config.initial_urls.is_empty() {
            let session = if running.settings.restore_session && running.profile_dir.is_some() {
                running.store.load_session()
            } else {
                nrb_profile::SessionState::default()
            };
            if session.tabs.is_empty() {
                let home = running.settings.homepage.clone();
                let start = Url::parse(&home)
                    .unwrap_or_else(|_| Url::parse("about:blank").expect("валидный URL"));
                let blank = start.as_str() == "about:blank";
                running.open_tab(start);
                running.toolbar.focus_url = blank;
            } else {
                for tab in &session.tabs {
                    if let Ok(url) = Url::parse(&tab.url) {
                        running.open_tab(url);
                    }
                }
                running.select_tab(session.active.min(running.tabs.len().saturating_sub(1)));
            }
        } else {
            for url in config.initial_urls {
                running.open_tab(url);
            }
            running.select_tab(0);
        }
        running.window.request_redraw();
        Ok(running)
    }

    /// Просит перерисовку и помнит откуда (диагностика простоя: `RUST_LOG=nrb_shell=debug`).
    #[track_caller]
    fn request_redraw(&self) {
        let at = std::panic::Location::caller();
        *self
            .shared
            .redraw_reasons
            .borrow_mut()
            .entry(format!("{}:{}", at.file().rsplit(['/', '\\']).next().unwrap_or(""), at.line()))
            .or_insert(0) += 1;
        self.redraw_requested.set(true);
        self.window.request_redraw();
    }

    fn content_size(&self) -> PhysicalSize<u32> {
        let size = self.window.inner_size();
        PhysicalSize::new(
            size.width.max(1),
            size.height.saturating_sub(self.toolbar_px).max(1),
        )
    }

    /// Подгоняет область страницы под окно. Контекст НЕ ресайзим напрямую: `WebView::resize` сам меняет размер контекста и
    /// обновляет viewport страницы, но делает это только если размер контекста ещё старый — иначе раскладка остаётся
    /// на размере запуска, а картинка обрезается.
    fn resize_content(&self) {
        let content = self.content_size();
        if self.tabs.is_empty() {
            self.content_ctx.resize(content);
        }
        for tab in &self.tabs {
            tab.resize(content);
        }
    }

    fn build_tab(&self, url: Url) -> WebView {
        // Пустая вкладка — страница новой вкладки в цветах темы (вместо белого about:blank).
        let url = if url.as_str() == "about:blank" {
            crate::start_page::url(&crate::theme::current(&self.egui_ctx))
        } else {
            url
        };
        let mut builder = WebViewBuilder::new(&self.servo, self.content_ctx.clone())
            .url(url)
            .hidpi_scale_factor(euclid_compat::scale(self.shared.scale.get()))
            .delegate(self.delegate.clone());
        if let Some(manager) = &self.user_content {
            builder = builder.user_content_manager(manager.clone());
        }
        builder.build()
    }

    fn open_tab(&mut self, url: Url) {
        let webview = self.build_tab(url);
        if (self.settings.default_zoom - 1.0).abs() > f32::EPSILON {
            webview.set_page_zoom(self.settings.default_zoom);
        }
        self.tabs.push(webview);
        self.select_tab(self.tabs.len() - 1);
    }

    fn adopt_tab(&mut self, webview: WebView) {
        self.tabs.push(webview);
        self.select_tab(self.tabs.len() - 1);
    }

    fn select_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        for (i, tab) in self.tabs.iter().enumerate() {
            if i == index {
                tab.show();
                tab.set_focused(true);
                tab.resize(self.content_size());
            } else {
                tab.set_focused(false);
                tab.hide();
            }
        }
        self.active = index;
        self.sync_url_bar(true);
        self.request_redraw();
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.exit = true;
            return;
        }
        let next = if index < self.active || self.active >= self.tabs.len() {
            self.active.saturating_sub(1)
        } else {
            self.active
        };
        self.select_tab(next.min(self.tabs.len() - 1));
    }

    fn active_tab(&self) -> Option<&WebView> {
        self.tabs.get(self.active)
    }

    fn sync_url_bar(&mut self, force: bool) {
        let editing = self
            .egui_ctx
            .memory(|m| m.has_focus(egui::Id::new("url-bar")));
        if editing && !force {
            return;
        }
        if let Some(tab) = self.active_tab() {
            let url = tab.url().map(|u| u.to_string()).unwrap_or_default();
            self.toolbar.url_input = if crate::start_page::is_blank(&url) {
                String::new()
            } else {
                url
            };
        }
    }

    /// Перехваченные загрузки: берём куки страницы из хранилища Servo и отдаём обработчику (клиент `nrb-net`, тот же транспорт).
    fn start_pending_downloads(&mut self) {
        let accepted: Vec<_> = self
            .shared
            .engine_downloads
            .borrow_mut()
            .drain(..)
            .collect();
        let mut changed = !accepted.is_empty();
        if let Some(handler) = self.downloads.clone() {
            for (incoming, mut stream) in accepted {
                let events = std::iter::from_fn(move || {
                    stream.blocking_recv().map(|event| match event {
                        DownloadEvent::Chunk(bytes) => BodyEvent::Chunk(bytes.to_vec()),
                        DownloadEvent::Done => BodyEvent::Done,
                        DownloadEvent::Failed(reason) => BodyEvent::Failed(reason),
                    })
                });
                handler.ingest(incoming, Box::new(events));
                self.downloads_window.open = true;
            }
        }
        let pending: Vec<(Url, Option<Url>)> = self
            .shared
            .download_requests
            .borrow_mut()
            .drain(..)
            .collect();
        let Some(handler) = self.downloads.clone() else {
            if changed {
                self.request_redraw();
            }
            return;
        };
        changed |= !pending.is_empty();
        for (url, referer) in pending {
            let cookies = self
                .servo
                .site_data_manager()
                .cookies_for_url(url.clone(), nrb_engine_servo::servo::CookieSource::HTTP)
                .iter()
                .map(|c| format!("{}={}", c.name(), c.value()))
                .collect::<Vec<_>>()
                .join("; ");
            handler.start(
                &url,
                (!cookies.is_empty()).then_some(cookies),
                referer.map(|r| r.to_string()),
            );
            self.downloads_window.open = true;
        }
        // Вызывается из `spin()` на каждое событие: без изменений перерисовку не просим (иначе цикл крутится вхолостую).
        if changed {
            self.request_redraw();
        }
    }

    fn start_print(&mut self) {
        if self.print_job.is_some() {
            return;
        }
        let (Some(tab), Some(handler)) = (self.active_tab().cloned(), self.downloads.clone())
        else {
            self.print_notice = Some((
                "Печать недоступна: нет каталога загрузок".into(),
                Instant::now() + Duration::from_secs(5),
            ));
            return;
        };
        self.print_job = Some(PrintJob::start(
            tab,
            Box::new(move |name| handler.reserve_file(name)),
        ));
    }

    /// Ход печати: сообщение в тулбаре; по завершении — итог на несколько секунд и открытие окна «Загрузки».
    fn poll_print(&mut self) {
        let Some(job) = self.print_job.as_mut() else {
            return;
        };
        match job.poll() {
            Progress::Running(text) => {
                self.print_notice = Some((text, Instant::now() + Duration::from_secs(2)));
            }
            Progress::Done(path) => {
                let note = if job.truncated() {
                    format!(
                        "PDF сохранён (первые {} экранов): {}",
                        crate::print::MAX_TILES,
                        path.display()
                    )
                } else {
                    format!("PDF сохранён: {}", path.display())
                };
                self.print_notice = Some((note, Instant::now() + Duration::from_secs(8)));
                self.print_job = None;
            }
            Progress::Failed(error) => {
                self.print_notice = Some((
                    format!("Печать не удалась: {error}"),
                    Instant::now() + Duration::from_secs(8),
                ));
                self.print_job = None;
            }
        }
        self.request_redraw();
    }

    fn spin(&mut self) {
        self.servo.spin_event_loop();
        self.poll_print();
        self.poll_audio();
        self.start_pending_downloads();
        let new_tabs: Vec<WebView> = self.shared.new_tabs.borrow_mut().drain(..).collect();
        for webview in new_tabs {
            self.adopt_tab(webview);
        }
        let incoming: Vec<_> = self.shared.modals.borrow_mut().drain(..).collect();
        self.modals.extend(incoming);
        let hidden: Vec<EmbedderControlId> =
            self.shared.hidden_controls.borrow_mut().drain(..).collect();
        for id in hidden {
            self.modals.retain(|(modal_id, _)| *modal_id != Some(id));
        }
        if let Some(fullscreen) = self.shared.fullscreen.take() {
            self.set_fullscreen(fullscreen);
        }
        let favicons: Vec<WebViewId> = self.shared.favicons.borrow_mut().drain(..).collect();
        for id in favicons {
            self.update_favicon(id);
        }
        self.record_history();
        self.save_session(false);
        let closed: Vec<WebView> = self.shared.closed.borrow_mut().drain(..).collect();
        for webview in closed {
            if let Some(index) = self.tabs.iter().position(|tab| tab.id() == webview.id()) {
                self.close_tab(index);
            }
        }
    }

    /// Журнал: посещения дозагрузившихся страниц и обновление заголовков.
    fn record_history(&mut self) {
        let completed: Vec<WebViewId> = self.shared.completed.borrow_mut().drain(..).collect();
        // Страница (пере)загрузилась — метки поиска исчезли вместе с прежним DOM.
        if let Some(active) = self.active_tab()
            && completed.contains(&active.id())
        {
            self.find_bar.reset_for_new_page();
        }
        let titled: Vec<WebViewId> = self.shared.titled.borrow_mut().drain(..).collect();
        if !self.settings.record_history || self.profile_dir.is_none() {
            return;
        }
        for id in completed {
            if let Some(tab) = self.tabs.iter().find(|tab| tab.id() == id)
                && let Some(url) = tab.url()
            {
                if crate::start_page::is_blank(url.as_str()) {
                    continue;
                }
                let title = tab.page_title().unwrap_or_default();
                let _ = self.store.record_visit(url.as_str(), &title);
            }
        }
        for id in titled {
            if let Some(tab) = self.tabs.iter().find(|tab| tab.id() == id)
                && let (Some(url), Some(title)) = (tab.url(), tab.page_title())
                && !crate::start_page::is_blank(url.as_str())
            {
                let _ = self.store.update_title(url.as_str(), &title);
            }
        }
    }

    /// Сессия вкладок на диск: не чаще раза в секунду и только при изменении; `force` — при выходе.
    fn save_session(&mut self, force: bool) {
        if self.profile_dir.is_none() {
            return;
        }
        let session = nrb_profile::SessionState {
            tabs: self
                .tabs
                .iter()
                .filter_map(|tab| {
                    tab.url().map(|url| nrb_profile::TabState {
                        url: if crate::start_page::is_blank(url.as_str()) {
                            "about:blank".to_owned()
                        } else {
                            url.to_string()
                        },
                        title: tab.page_title().unwrap_or_default(),
                    })
                })
                .collect(),
            active: self.active,
        };
        if session == self.saved_session.0
            || (!force && self.saved_session.1.elapsed() < std::time::Duration::from_secs(1))
        {
            return;
        }
        if let Err(error) = self.store.save_session(&session) {
            tracing::warn!(%error, "не удалось сохранить сессию");
        }
        self.saved_session = (session, std::time::Instant::now());
    }

    /// Подсказки адресной строки: закладки и история по введённому тексту (не более 6).
    fn refresh_suggestions(&mut self) {
        let query = self.toolbar.url_input.trim().to_owned();
        let current = self
            .active_tab()
            .and_then(|t| t.url())
            .map(|u| u.to_string())
            .unwrap_or_default();
        self.toolbar.suggestions.clear();
        if query.is_empty() || query == current || self.profile_dir.is_none() {
            return;
        }
        let needle = query.to_lowercase();
        let mut seen = std::collections::HashSet::new();
        if let Ok(bookmarks) = self.store.bookmarks() {
            for bookmark in bookmarks {
                if (bookmark.url.to_lowercase().contains(&needle)
                    || bookmark.title.to_lowercase().contains(&needle))
                    && seen.insert(bookmark.url.clone())
                {
                    self.toolbar
                        .suggestions
                        .push((bookmark.url, bookmark.title));
                }
            }
        }
        if let Ok(entries) = self.store.search_history(&query, 6) {
            for entry in entries {
                if seen.insert(entry.url.clone()) {
                    self.toolbar.suggestions.push((entry.url, entry.title));
                }
            }
        }
        self.toolbar.suggestions.truncate(6);
    }

    /// Выполнить последствия кадра менеджера паролей: подстановка в страницу и короткие сообщения.
    fn run_password_effects(&mut self, effects: Vec<PwEffect>) {
        if effects.is_empty() {
            return;
        }
        for effect in effects {
            match effect {
                PwEffect::Fill {
                    tab,
                    username,
                    password,
                } => {
                    if let Some(tab) = self.tabs.get(tab) {
                        tab.evaluate_javascript(
                            passwords::fill_script(&username, &password),
                            |_| {},
                        );
                    }
                }
                PwEffect::Notice(text) => {
                    self.print_notice = Some((text, Instant::now() + Duration::from_secs(4)));
                }
            }
        }
        self.request_redraw();
    }

    /// Скрипт настройки звука вкладки: политика автовоспроизведения из настроек и признак «заглушена».
    fn audio_config_script(&self, id: WebViewId) -> String {
        format!(
            "window.__nrbAudio && window.__nrbAudio.config({{blockAutoplay: {}, tabMuted: {}}})",
            !self.settings.autoplay_with_sound,
            self.muted_tabs.contains(&id)
        )
    }

    fn apply_audio_config(&self, tab: &WebView) {
        tab.evaluate_javascript(self.audio_config_script(tab.id()), |_| {});
    }

    fn toggle_mute(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index).cloned() else {
            return;
        };
        if !self.muted_tabs.remove(&tab.id()) {
            self.muted_tabs.insert(tab.id());
        }
        self.apply_audio_config(&tab);
        self.request_redraw();
    }

    /// Раз в 1,5 с спрашивает у каждой вкладки, звучит ли она, и применяет принесённые ответы.
    fn poll_audio(&mut self) {
        let incoming: Vec<_> = self.passwords.inbox.borrow_mut().drain(..).collect();
        for (id, json) in incoming {
            self.passwords.ingest(id, &json);
        }
        let apply: Vec<WebViewId> = self.shared.audio_apply.borrow_mut().drain(..).collect();
        for id in apply {
            if let Some(tab) = self.tabs.iter().find(|t| t.id() == id) {
                self.apply_audio_config(tab);
            }
        }
        for (id, playing) in self.shared.audio_inbox.borrow_mut().drain(..) {
            if playing {
                self.audible_tabs.insert(id);
            } else {
                self.audible_tabs.remove(&id);
            }
        }
        let now = std::time::Instant::now();
        if now < self.next_audio_poll {
            return;
        }
        self.next_audio_poll = now + std::time::Duration::from_millis(1500);
        if self.passwords.available() {
            for tab in &self.tabs {
                let (id, inbox) = (tab.id(), self.passwords.inbox.clone());
                tab.evaluate_javascript(passwords::POLL_SCRIPT, move |result| {
                    if let Ok(nrb_engine_servo::servo::JSValue::String(json)) = result
                        && !json.is_empty()
                    {
                        inbox.borrow_mut().push((id, json));
                    }
                });
            }
        }
        for tab in &self.tabs {
            let (id, inbox) = (tab.id(), self.shared.audio_inbox.clone());
            tab.evaluate_javascript(
                "window.__nrbAudio ? window.__nrbAudio.state() : ''",
                move |result| {
                    let playing = matches!(
                        result,
                        Ok(nrb_engine_servo::servo::JSValue::String(json)) if json.contains("\"playing\":true")
                    );
                    inbox.borrow_mut().push((id, playing));
                },
            );
        }
        self.request_redraw();
    }

    /// Перечитывает запомненные решения из профиля (после сброса в настройках).
    fn reload_permissions(&mut self) {
        *self.shared.permissions.borrow_mut() = load_permissions(&self.store);
    }

    fn apply_settings(&mut self) {
        self.search = self.settings.search.clone();
        self.shared.adblock_on.set(self.settings.adblock);
        apply_engine_policy(&self.settings, self.webrtc_forbidden);
        self.passwords.enabled = self.settings.passwords && self.profile_dir.is_some();
        for tab in &self.tabs {
            self.apply_audio_config(tab);
        }
        crate::theme::apply(&self.egui_ctx, self.settings.theme);
        if let Some(dir) = &self.profile_dir
            && let Err(error) = self.settings.save(dir)
        {
            tracing::warn!(%error, "не удалось сохранить настройки");
        }
    }

    fn zoom(&self, change: impl Fn(f32) -> f32) {
        if let Some(tab) = self.active_tab() {
            tab.set_page_zoom(change(tab.page_zoom()).clamp(0.3, 5.0));
        }
    }

    fn toggle_bookmark(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        let Some(url) = tab.url() else { return };
        let title = tab.page_title().unwrap_or_default();
        if self.profile_dir.is_none() {
            return; // приватный сеанс: закладки не сохраняются
        }
        let _ = self.store.toggle_bookmark(url.as_str(), &title);
        self.bookmark_cache.0.clear(); // пересчитать звезду
        self.request_redraw();
    }

    fn set_fullscreen(&mut self, fullscreen: bool) {
        self.fullscreen = fullscreen;
        self.window
            .set_fullscreen(fullscreen.then_some(winit::window::Fullscreen::Borderless(None)));
        self.request_redraw();
    }

    fn update_favicon(&mut self, id: WebViewId) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id() == id) else {
            return;
        };
        let Some(image) = tab.favicon() else { return };
        let (width, height) = (image.width as usize, image.height as usize);
        let data = image.data();
        let color = match image.format {
            PixelFormat::RGBA8 if data.len() >= width * height * 4 => {
                egui::ColorImage::from_rgba_unmultiplied(
                    [width, height],
                    &data[..width * height * 4],
                )
            }
            PixelFormat::RGB8 if data.len() >= width * height * 3 => {
                egui::ColorImage::from_rgb([width, height], &data[..width * height * 3])
            }
            _ => return,
        };
        drop(image);
        let texture = self.egui_ctx.load_texture(
            format!("favicon-{id:?}"),
            color,
            egui::TextureOptions::LINEAR,
        );
        self.favicons.insert(id, texture);
        self.request_redraw();
    }

    fn navigate(&mut self, input: &str) {
        if let (Some(url), Some(tab)) = (omnibox::resolve(input, &self.search), self.active_tab()) {
            tab.load(url);
            tab.set_focused(true);
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Navigate(input) => self.navigate(&input),
            Action::Back => {
                if let Some(tab) = self.active_tab() {
                    tab.go_back(1);
                }
            }
            Action::Forward => {
                if let Some(tab) = self.active_tab() {
                    tab.go_forward(1);
                }
            }
            Action::Reload => {
                if let Some(tab) = self.active_tab() {
                    tab.reload();
                }
            }
            Action::NewTab => {
                if let Ok(url) = Url::parse("about:blank") {
                    self.open_tab(url);
                    self.toolbar.focus_url = true;
                }
            }
            Action::CloseTab(index) => self.close_tab(index),
            Action::SelectTab(index) => self.select_tab(index),
            Action::ToggleMute(index) => self.toggle_mute(index),
            Action::OpenPasswords => self.passwords.toggle_window(),
            Action::FillPassword => {
                let origin = self
                    .active_tab()
                    .and_then(|t| t.url())
                    .and_then(|u| passwords::origin_of(&u));
                let effects = self.passwords.request_fill(self.active, origin.as_deref());
                self.run_password_effects(effects);
            }
            Action::ToggleBookmark => self.toggle_bookmark(),
            Action::OpenHistory => self.library.toggle(Panel::History),
            Action::OpenBookmarks => self.library.toggle(Panel::Bookmarks),
            Action::OpenSettings => self.library.toggle(Panel::Settings),
            Action::OpenNodes => self.nodes_window.toggle(),
            Action::OpenDownloads => self.downloads_window.toggle(),
            Action::Find => self.find_bar.open(),
            Action::OpenSiteData => self.site_data_window.toggle(),
            Action::Print => self.start_print(),
            Action::ZoomIn => self.zoom(|z| z * 1.1),
            Action::ZoomOut => self.zoom(|z| z / 1.1),
            Action::ZoomReset => {
                let default = self.settings.default_zoom;
                self.zoom(|_| default);
            }
        }
    }

    /// Горячие клавиши оболочки. `true` — клавиша обработана и дальше не идёт.
    fn shortcut(&mut self, key: &WKey, state: ElementState) -> bool {
        if state != ElementState::Pressed || !self.modals.is_empty() {
            return false;
        }
        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        let shift = self.modifiers.shift_key();
        let character = match key {
            WKey::Character(text) => text.to_lowercase(),
            _ => String::new(),
        };
        match key {
            // Android: системная кнопка «Назад» — назад по истории, а без истории сворачивает приложение (выход из цикла).
            WKey::Named(WNamedKey::GoBack) => match self.active_tab() {
                Some(tab) if tab.can_go_back() => self.apply(Action::Back),
                _ => self.exit = true,
            },
            WKey::Named(WNamedKey::ArrowLeft) if alt => self.apply(Action::Back),
            WKey::Named(WNamedKey::ArrowRight) if alt => self.apply(Action::Forward),
            WKey::Named(WNamedKey::F5) => self.apply(Action::Reload),
            WKey::Named(WNamedKey::Delete) if ctrl && shift => self.apply(Action::OpenSiteData),
            WKey::Named(WNamedKey::F11) => self.set_fullscreen(!self.fullscreen),
            WKey::Named(WNamedKey::Escape) if self.fullscreen && self.modals.is_empty() => {
                if let Some(tab) = self.active_tab() {
                    tab.exit_fullscreen();
                }
                self.set_fullscreen(false);
            }
            WKey::Named(WNamedKey::F6) => self.toolbar.focus_url = true,
            WKey::Named(WNamedKey::Tab) if ctrl => {
                let count = self.tabs.len().max(1);
                let next = if shift {
                    (self.active + count - 1) % count
                } else {
                    (self.active + 1) % count
                };
                self.select_tab(next);
            }
            _ if ctrl && character == "d" => self.apply(Action::ToggleBookmark),
            _ if ctrl && character == "h" => self.apply(Action::OpenHistory),
            _ if ctrl && shift && character == "o" => self.apply(Action::OpenBookmarks),
            _ if ctrl && character == "," => self.apply(Action::OpenSettings),
            _ if ctrl && shift && character == "n" => self.apply(Action::OpenNodes),
            _ if ctrl && !shift && character == "j" => self.apply(Action::OpenDownloads),
            _ if ctrl && !shift && character == "f" => self.apply(Action::Find),
            _ if ctrl && !shift && character == "p" => self.apply(Action::Print),
            _ if ctrl && (character == "=" || character == "+") => self.apply(Action::ZoomIn),
            _ if ctrl && (character == "-" || character == "_") => self.apply(Action::ZoomOut),
            _ if ctrl && character == "0" => self.apply(Action::ZoomReset),
            _ if ctrl && character == "t" => self.apply(Action::NewTab),
            _ if ctrl && character == "w" => self.apply(Action::CloseTab(self.active)),
            _ if ctrl && shift && character == "p" => self.apply(Action::OpenPasswords),
            _ if ctrl && shift && character == "l" => self.apply(Action::FillPassword),
            // Ctrl+M — заглушить/вернуть звук активной вкладки (как в Firefox).
            _ if ctrl && !shift && character == "m" => self.apply(Action::ToggleMute(self.active)),
            _ if ctrl && character == "l" => self.toolbar.focus_url = true,
            _ if ctrl && character == "r" => self.apply(Action::Reload),
            _ => return false,
        }
        self.request_redraw();
        true
    }

    /// Курсор над страницей и под ним нет модального окна (пока окно открыто, страница ввода не получает).
    fn over_content(&self) -> bool {
        self.modals.is_empty()
            && !self.egui_ctx.egui_wants_pointer_input()
            && self.cursor.y >= f64::from(self.toolbar_px)
    }

    fn content_point(&self) -> WebViewPoint {
        WebViewPoint::Device(DevicePoint::new(
            self.cursor.x as f32,
            (self.cursor.y - f64::from(self.toolbar_px)) as f32,
        ))
    }

    fn on_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        self.spin();

        // Горячие клавиши — до egui, чтобы Ctrl+L/T/W работали и при фокусе в адресной строке.
        if let WindowEvent::KeyboardInput {
            event: key_event, ..
        } = &event
            && self.shortcut(&key_event.logical_key, key_event.state)
        {
            return;
        }
        let response = self.egui_state.on_window_event(&self.window, &event);
        // `RedrawRequested` egui всегда отвечает «перерисуй»: если на это просить ещё кадр, цикл перерисовки не останавливается
        // никогда (165 кадров/с на простое). Следующий кадр просит сам `redraw()`, когда egui или страница анимируются.
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            self.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => {
                self.save_session(true);
                event_loop.exit();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Resized(size) => {
                self.window_ctx.resize(size);
                self.resize_content();
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.shared.scale.set(scale_factor as f32);
                self.toolbar_px = (68.0 * scale_factor as f32).round() as u32;
                for tab in &self.tabs {
                    tab.set_hidpi_scale_factor(euclid_compat::scale(scale_factor as f32));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                if self.over_content()
                    && let Some(tab) = self.active_tab()
                {
                    tab.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(
                        self.content_point(),
                    )));
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.over_content()
                    && let Some(tab) = self.active_tab()
                {
                    let button = match button {
                        WMouseButton::Left => MouseButton::Primary,
                        WMouseButton::Right => MouseButton::Secondary,
                        WMouseButton::Middle => MouseButton::Auxiliary,
                        WMouseButton::Back => MouseButton::Back,
                        WMouseButton::Forward => MouseButton::Forward,
                        WMouseButton::Other(n) => MouseButton::Other(n),
                    };
                    let action = match state {
                        ElementState::Pressed => MouseButtonAction::Down,
                        ElementState::Released => MouseButtonAction::Up,
                    };
                    tab.set_focused(true);
                    tab.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                        action,
                        button,
                        self.content_point(),
                    )));
                }
            }
            WindowEvent::Touch(touch) => {
                // Тач в области страницы — в Servo (прокрутка, касание → клик, жесты); по панели инструментов — egui (он уже
                // получил событие выше и превращает его в указатель).
                if touch.location.y >= f64::from(self.toolbar_px)
                    && self.modals.is_empty()
                    && !self.egui_ctx.egui_wants_pointer_input()
                    && let Some(tab) = self.active_tab()
                {
                    let event_type = match touch.phase {
                        winit::event::TouchPhase::Started => TouchEventType::Down,
                        winit::event::TouchPhase::Moved => TouchEventType::Move,
                        winit::event::TouchPhase::Ended => TouchEventType::Up,
                        winit::event::TouchPhase::Cancelled => TouchEventType::Cancel,
                    };
                    let point = WebViewPoint::Device(DevicePoint::new(
                        touch.location.x as f32,
                        (touch.location.y - f64::from(self.toolbar_px)) as f32,
                    ));
                    if matches!(event_type, TouchEventType::Down) {
                        tab.set_focused(true);
                    }
                    tab.notify_input_event(InputEvent::Touch(TouchEvent::new(
                        event_type,
                        TouchId(touch.id as i32),
                        point,
                        TouchPointerType::Touch,
                    )));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.over_content()
                    && let Some(tab) = self.active_tab()
                {
                    match delta {
                        // Колесо с щелчками (Ctrl — масштаб страницы — отдаём сразу): копим и отдаём движку порциями
                        // по кадрам (`tick_smooth_scroll`), как браузеры делают плавную прокрутку.
                        MouseScrollDelta::LineDelta(dx, dy) if !self.modifiers.control_key() => {
                            if self.wheel_pending == (0.0, 0.0) {
                                self.wheel_tick = Instant::now();
                            }
                            self.wheel_pending.0 += f64::from(dx) * 76.0;
                            self.wheel_pending.1 += f64::from(dy) * 76.0;
                            self.request_redraw();
                        },
                        MouseScrollDelta::LineDelta(dx, dy) => {
                            tab.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                                WheelDelta {
                                    x: f64::from(dx) * 76.0,
                                    y: f64::from(dy) * 76.0,
                                    z: 0.0,
                                    mode: WheelMode::DeltaLine,
                                },
                                self.content_point(),
                            )));
                        },
                        // Тачпад уже шлёт мелкие пиксельные шаги.
                        MouseScrollDelta::PixelDelta(delta) => {
                            tab.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                                WheelDelta {
                                    x: delta.x,
                                    y: delta.y,
                                    z: 0.0,
                                    mode: WheelMode::DeltaPixel,
                                },
                                self.content_point(),
                            )));
                        },
                    }
                }
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if self.modals.is_empty()
                    && !self.egui_ctx.egui_wants_keyboard_input()
                    && let Some(tab) = self.active_tab()
                {
                    tab.notify_input_event(InputEvent::Keyboard(ServoKeyboardEvent::new(
                        keys::keyboard_event(&key_event, self.modifiers),
                    )));
                }
            }
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                if self.modals.is_empty()
                    && !self.egui_ctx.egui_wants_keyboard_input()
                    && let Some(tab) = self.active_tab()
                {
                    tab.notify_input_event(InputEvent::Ime(ImeEvent::Composition(
                        CompositionEvent {
                            state: CompositionState::End,
                            data: text,
                        },
                    )));
                }
            }
            WindowEvent::RedrawRequested if self.suspended => {}
            WindowEvent::RedrawRequested => {
                if !self.redraw_requested.replace(false) {
                    *self
                        .shared
                        .redraw_reasons
                        .borrow_mut()
                        .entry("ОС (WM_PAINT)".to_owned())
                        .or_insert(0) += 1;
                }
                self.redraw()
            },
            _ => {}
        }
    }

    /// Копирует offscreen-кадр Servo в нижнюю часть окна. Свой блит с явным состоянием GL: готовый
    /// `render_to_parent_callback` Servo в этой конфигурации ничего не копировал.
    fn blit_content(&self) {
        use egui_glow::glow::{self, HasContext as _};
        let content = self.content_size();
        let gl = &self.gl;
        let bound =
            |gl: &glow::Context| unsafe { gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING) as u32 };
        let framebuffer = |id: u32| std::num::NonZeroU32::new(id).map(glow::NativeFramebuffer);
        self.content_ctx.prepare_for_rendering();
        let src = bound(gl);
        self.window_ctx.prepare_for_rendering();
        let dst = bound(gl);
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.color_mask(true, true, true, true);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, framebuffer(src));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, framebuffer(dst));
            // Низ окна: GL-координаты считаются снизу, поэтому область страницы — [0, высота_страницы].
            gl.blit_framebuffer(
                0,
                0,
                content.width as i32,
                content.height as i32,
                0,
                0,
                content.width as i32,
                content.height as i32,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, framebuffer(dst));
        }
    }

    /// Плавная прокрутка колесом: накопленную дельту отдаём движку долями на каждом кадре (экспоненциальное
    /// затухание, постоянная времени ~45 мс — до 95% за ~135 мс). Рывок на целый щелчок в один кадр был «дёрганьем».
    fn tick_smooth_scroll(&mut self) {
        if self.wheel_pending == (0.0, 0.0) {
            return;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.wheel_tick).as_secs_f64().min(0.1);
        self.wheel_tick = now;
        let share = 1.0 - (-dt / 0.045).exp();
        let step = |pending: f64| {
            let part = pending * share;
            // Остаток меньше пикселя — отдаём целиком, чтобы анимация закончилась.
            if (pending - part).abs() < 0.5 { pending } else { part }
        };
        let (px, py) = (step(self.wheel_pending.0), step(self.wheel_pending.1));
        self.wheel_pending.0 -= px;
        self.wheel_pending.1 -= py;
        if (px, py) == (0.0, 0.0) {
            return;
        }
        if self.over_content()
            && let Some(tab) = self.active_tab()
        {
            tab.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                WheelDelta {
                    x: px,
                    y: py,
                    z: 0.0,
                    mode: WheelMode::DeltaPixel,
                },
                self.content_point(),
            )));
        } else {
            self.wheel_pending = (0.0, 0.0);
        }
    }

    fn redraw(&mut self) {
        use egui_glow::glow::HasContext as _;
        self.tick_smooth_scroll();
        let frame_started = Instant::now();
        self.frame_stats.gap_max = self
            .frame_stats
            .gap_max
            .max(frame_started.duration_since(self.frame_stats.last_end));
        self.window_ctx.prepare_for_rendering();
        let size = self.window.inner_size();
        let scale = self.window.scale_factor() as f32;

        self.toolbar.tunnel = self.tunnel.as_ref().map(|t| (t.summary(), t.up()));
        self.toolbar.notice = self
            .print_notice
            .as_ref()
            .filter(|(_, until)| *until > Instant::now())
            .map(|(text, _)| text.clone());
        self.toolbar.blocked = (self.shared.blocker.is_some() && self.shared.adblock_on.get())
            .then(|| self.shared.blocked.get());
        let current_url = self
            .active_tab()
            .and_then(|t| t.url())
            .map(|u| u.to_string())
            .unwrap_or_default();
        if self.bookmark_cache.0 != current_url {
            let marked = self.profile_dir.is_some()
                && self.store.is_bookmarked(&current_url).unwrap_or(false);
            self.bookmark_cache = (current_url, marked);
        }
        self.toolbar.bookmarked = self.bookmark_cache.1;
        self.toolbar.login_form = self.passwords.available()
            && self
                .active_tab()
                .is_some_and(|t| self.passwords.login_tabs.contains(&t.id()));
        if self.toolbar.input_changed {
            self.toolbar.input_changed = false;
            self.refresh_suggestions();
        }

        // Состояние тулбара из активной вкладки.
        self.sync_url_bar(false);
        let tabs: Vec<TabInfo> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| TabInfo {
                icon: self.favicons.get(&tab.id()).map(|t| t.id()),
                title: tab
                    .page_title()
                    .filter(|t| !t.is_empty())
                    .or_else(|| {
                        tab.url().map(|u| {
                            u.host_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| u.to_string())
                        })
                    })
                    .unwrap_or_else(|| "Новая вкладка".into()),
                active: index == self.active,
                loading: index == self.active && tab.load_status() != LoadStatus::Complete,
                audio: if self.muted_tabs.contains(&tab.id()) {
                    TabAudioState::Muted
                } else if self.audible_tabs.contains(&tab.id()) {
                    TabAudioState::Playing
                } else {
                    TabAudioState::Silent
                },
            })
            .collect();
        if let Some(tab) = self.tabs.get(self.active) {
            self.toolbar.can_back = tab.can_go_back();
            self.toolbar.can_forward = tab.can_go_forward();
            self.toolbar.loading = tab.load_status() != LoadStatus::Complete;
            let title = tab
                .page_title()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "nrb".into());
            self.window.set_title(&format!("{title} — nrb"));
        }

        unsafe {
            self.gl.disable(egui_glow::glow::SCISSOR_TEST);
            self.gl
                .viewport(0, 0, size.width as i32, size.height as i32);
            self.gl.clear_color(0.12, 0.12, 0.13, 1.0);
            self.gl.clear(egui_glow::glow::COLOR_BUFFER_BIT);
        }

        // Страница: Servo рисует в offscreen, затем блит в нижнюю часть окна; тулбар egui рисуется поверх верхней.
        let phase = Instant::now();
        if let Some(tab) = self.tabs.get(self.active) {
            tab.paint();
            self.blit_content();
            self.window.set_cursor(cursor_icon(tab.cursor()));
        }
        self.frame_stats.page.add(phase.elapsed());
        let phase = Instant::now();

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let (active_index, active_origin) = (
            self.active,
            self.active_tab()
                .and_then(|t| t.url())
                .and_then(|u| passwords::origin_of(&u)),
        );
        let mut actions = Vec::new();
        let mut content_top = 0.0_f32;
        let toolbar = &mut self.toolbar;
        let fullscreen = self.fullscreen;
        let mut modal_done = false;
        let mut library_actions = Vec::new();
        let mut nodes_restart = false;
        let mut find_actions = Vec::new();
        let mut password_effects = Vec::new();
        let mut output = self.egui_ctx.clone().run_ui(raw_input, |ui| {
            if !fullscreen {
                actions = ui::toolbar(ui, &tabs, toolbar);
                find_actions = self.find_bar.show(ui);
                password_effects = self.passwords.show(ui, active_index, active_origin.clone());
                content_top = ui.available_rect_before_wrap().top();
            }
            library_actions = self.library.show(
                ui.ctx(),
                &self.store,
                &mut self.settings,
                !self.webrtc_forbidden,
            );
            if let Some(handler) = self.downloads.clone() {
                self.downloads_window.show(ui.ctx(), handler.as_ref());
            }
            self.site_data_window
                .show(ui.ctx(), self.servo.site_data_manager());
            if let Some(manager) = self.nodes.clone() {
                let summary = self.tunnel.as_ref().map(|t| t.summary());
                nodes_restart = self.nodes_window.show(ui.ctx(), manager.as_ref(), summary);
            }
            if let Some((_, modal)) = self.modals.front_mut() {
                modal_done = modal::show(ui.ctx(), modal);
            }
        });
        for action in library_actions {
            match action {
                LibraryAction::Open(url) => self.navigate(&url),
                LibraryAction::SettingsChanged => self.apply_settings(),
                LibraryAction::PermissionsChanged => self.reload_permissions(),
            }
        }
        self.run_password_effects(password_effects);
        for action in find_actions {
            let Some(tab) = self.active_tab() else {
                continue;
            };
            let (query, case_sensitive) = {
                let (query, case) = self.find_bar.query();
                (query.to_owned(), case)
            };
            let request = match action {
                FindAction::Search(query, case_sensitive) => FindRequest {
                    action: EngineFindAction::Search,
                    query,
                    case_sensitive,
                },
                FindAction::Step(direction) => FindRequest {
                    action: if direction < 0 {
                        EngineFindAction::Previous
                    } else {
                        EngineFindAction::Next
                    },
                    query,
                    case_sensitive,
                },
                FindAction::Close => FindRequest {
                    action: EngineFindAction::Clear,
                    query,
                    case_sensitive,
                },
            };
            let wants_result = request.action != EngineFindAction::Clear;
            let shared = self.find_bar.shared.clone();
            tab.find(request, move |result| {
                if wants_result
                    && let Ok(nrb_engine_servo::servo::JSValue::String(json)) = result
                    && let Some(parsed) = find::parse_result(&json)
                {
                    *shared.borrow_mut() = Some(parsed);
                }
            });
        }
        if nodes_restart && let Some(manager) = self.nodes.clone() {
            self.save_session(true);
            manager.request_restart();
            self.exit = true;
        }
        if modal_done {
            if let Some((
                _,
                Modal::Permission {
                    key: Some(key),
                    remember: true,
                    decision: Some(allow),
                    ..
                },
            )) = self.modals.front()
            {
                if let Err(error) = self.store.set_permission(&key.0, &key.1, *allow) {
                    tracing::warn!("не удалось сохранить решение о разрешении: {error}");
                }
                self.shared
                    .permissions
                    .borrow_mut()
                    .insert(key.clone(), *allow);
            }
            self.modals.pop_front();
            self.request_redraw();
        }
        self.egui_state
            .handle_platform_output(&self.window, output.platform_output.clone());
        let primitives = self
            .egui_ctx
            .tessellate(output.shapes.clone(), output.pixels_per_point);
        self.painter.paint_and_update_textures(
            [size.width, size.height],
            output.pixels_per_point,
            &primitives,
            &mut output.textures_delta,
        );

        self.frame_stats.egui.add(phase.elapsed());

        // Фактическая высота тулбара (в пикселях) → размер области страницы.
        let toolbar_px = if self.fullscreen {
            0
        } else {
            (content_top * scale).round().max(1.0) as u32
        };
        if toolbar_px != self.toolbar_px {
            self.toolbar_px = toolbar_px;
            self.shared.toolbar_px.set(toolbar_px);
            self.resize_content();
        }

        let phase = Instant::now();
        self.window_ctx.present();
        self.frame_stats.present.add(phase.elapsed());

        for action in actions {
            self.apply(action);
        }
        let egui_repaint = self.egui_ctx.has_requested_repaint();
        let page_animating = self
            .tabs
            .get(self.active)
            .is_some_and(|tab| tab.animating());
        // Диагностика простоя: сколько кадров в секунду и что просит следующий (`RUST_LOG=nrb_shell=debug`).
        self.frames += 1;
        self.frames_repaint_egui += u32::from(egui_repaint);
        self.frames_page_animating += u32::from(page_animating);
        if self.frames_since.elapsed() >= Duration::from_secs(2) {
            let secs = self.frames_since.elapsed().as_secs_f32();
            let mut why: Vec<_> = self.shared.redraw_reasons.borrow_mut().drain().collect();
            why.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            tracing::debug!(
                target: "nrb_shell::redraw",
                "кадров/с {:.1} (egui просит {}, анимация страницы {}); мс на кадр: страница {}, egui {}, present {}; макс. пауза между кадрами {:.0} мс; просили: {:?}",
                self.frames as f32 / secs,
                self.frames_repaint_egui,
                self.frames_page_animating,
                self.frame_stats.page.take(),
                self.frame_stats.egui.take(),
                self.frame_stats.present.take(),
                std::mem::take(&mut self.frame_stats.gap_max).as_secs_f64() * 1000.0,
                why
            );
            self.frames = 0;
            self.frames_repaint_egui = 0;
            self.frames_page_animating = 0;
            self.frames_since = Instant::now();
        }
        self.frame_stats.last_end = Instant::now();
        if egui_repaint || page_animating || self.wheel_pending != (0.0, 0.0) {
            self.request_redraw();
        }
    }
}

fn cursor_icon(cursor: Cursor) -> CursorIcon {
    match cursor {
        Cursor::None | Cursor::Default => CursorIcon::Default,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::ContextMenu => CursorIcon::ContextMenu,
        Cursor::Help => CursorIcon::Help,
        Cursor::Progress => CursorIcon::Progress,
        Cursor::Wait => CursorIcon::Wait,
        Cursor::Cell => CursorIcon::Cell,
        Cursor::Crosshair => CursorIcon::Crosshair,
        Cursor::Text => CursorIcon::Text,
        Cursor::VerticalText => CursorIcon::VerticalText,
        Cursor::Alias => CursorIcon::Alias,
        Cursor::Copy => CursorIcon::Copy,
        Cursor::Move => CursorIcon::Move,
        Cursor::NoDrop => CursorIcon::NoDrop,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::EResize => CursorIcon::EResize,
        Cursor::NResize => CursorIcon::NResize,
        Cursor::NeResize => CursorIcon::NeResize,
        Cursor::NwResize => CursorIcon::NwResize,
        Cursor::SResize => CursorIcon::SResize,
        Cursor::SeResize => CursorIcon::SeResize,
        Cursor::SwResize => CursorIcon::SwResize,
        Cursor::WResize => CursorIcon::WResize,
        Cursor::EwResize => CursorIcon::EwResize,
        Cursor::NsResize => CursorIcon::NsResize,
        Cursor::NeswResize => CursorIcon::NeswResize,
        Cursor::NwseResize => CursorIcon::NwseResize,
        Cursor::ColResize => CursorIcon::ColResize,
        Cursor::RowResize => CursorIcon::RowResize,
        Cursor::AllScroll => CursorIcon::AllScroll,
        Cursor::ZoomIn => CursorIcon::ZoomIn,
        Cursor::ZoomOut => CursorIcon::ZoomOut,
    }
}
