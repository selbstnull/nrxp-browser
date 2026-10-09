use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use nrb_engine::omnibox;
use nrb_engine_servo::servo::{
    AllowOrDenyRequest, AuthenticationRequest, CompositionEvent, CompositionState,
    CreateNewWebViewRequest, Cursor, DeviceIntRect, DevicePoint, EmbedderControl,
    EmbedderControlId, EventLoopWaker, ImeEvent, InputEvent, KeyboardEvent as ServoKeyboardEvent,
    LoadStatus, MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    OffscreenRenderingContext, PermissionRequest, PixelFormat, RenderingContext, Servo,
    ServoBuilder, ServoDelegate, SimpleDialog, WebResourceLoad, WebResourceResponse, WebView,
    WebViewBuilder, WebViewDelegate, WebViewId, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
    WindowRenderingContext,
};
use nrb_engine_servo::servo::{UserContentManager, UserScript};
use nrb_engine_servo::{ServoConfig, build_opts, build_preferences, install_crypto_provider};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton as WMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey as WNamedKey};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{CursorIcon, Window, WindowId};

use crate::keys;
use crate::library::{Library, LibraryAction, Panel};
use crate::modal::{self, Modal};
use crate::nodes::{NodeManager, NodesWindow};
use crate::ui::{self, Action, TabInfo, ToolbarState};

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
            adblock: None,
            userscripts: Vec::new(),
        }
    }
}

pub fn run(config: ShellConfig) -> anyhow::Result<()> {
    install_crypto_provider();
    let event_loop = EventLoop::<WakerEvent>::with_user_event().build()?;
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
        if let Some(window) = self.window.borrow().as_ref() {
            window.request_redraw();
        }
    }
}

struct Delegate(Rc<Shared>);

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.0.redraw();
    }

    fn notify_url_changed(&self, _webview: WebView, _url: Url) {
        self.0.redraw();
    }

    fn notify_page_title_changed(&self, webview: WebView, _title: Option<String>) {
        self.0.titled.borrow_mut().push(webview.id());
        self.0.redraw();
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        if status == LoadStatus::Complete {
            self.0.completed.borrow_mut().push(webview.id());
        }
        self.0.redraw();
    }

    /// Блокировка рекламы и трекеров до сети. Главный документ не трогаем; страница верхнего уровня — источник для `$third-party`.
    fn load_web_resource(&self, webview: WebView, load: WebResourceLoad) {
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
            self.0.redraw();
        }
    }

    fn notify_history_changed(&self, _webview: WebView, _entries: Vec<Url>, _current: usize) {
        self.0.redraw();
    }

    fn notify_cursor_changed(&self, _webview: WebView, _cursor: Cursor) {
        self.0.redraw();
    }

    fn notify_favicon_changed(&self, webview: WebView) {
        self.0.favicons.borrow_mut().push(webview.id());
        self.0.redraw();
    }

    fn notify_fullscreen_state_changed(&self, _webview: WebView, fullscreen: bool) {
        self.0.fullscreen.set(Some(fullscreen));
        self.0.redraw();
    }

    fn request_permission(&self, webview: WebView, request: PermissionRequest) {
        let origin = origin_of(&webview);
        let feature = request.feature();
        self.0.modals.borrow_mut().push((
            None,
            Modal::Permission {
                origin,
                feature,
                request: Some(request),
            },
        ));
        self.0.redraw();
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
        self.0.redraw();
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
        self.0.redraw();
    }

    fn hide_embedder_control(&self, _webview: WebView, id: EmbedderControlId) {
        self.0.hidden_controls.borrow_mut().push(id);
        self.0.redraw();
    }

    fn notify_closed(&self, webview: WebView) {
        self.0.closed.borrow_mut().push(webview);
        self.0.redraw();
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
        self.0.redraw();
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
    next_badge_refresh: std::time::Instant,
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

    /// Бейдж туннеля меняется без событий окна — раз в секунду просим перерисовку.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let App::Running(running) = self
            && running.tunnel.is_some()
        {
            let now = std::time::Instant::now();
            if now >= running.next_badge_refresh {
                running.next_badge_refresh = now + std::time::Duration::from_secs(1);
                running.window.request_redraw();
            }
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                running.next_badge_refresh,
            ));
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
        let user_content = (!config.userscripts.is_empty()).then(|| {
            let manager = Rc::new(UserContentManager::new(&servo));
            for js in &config.userscripts {
                manager.add_script(Rc::new(UserScript::new(js.clone(), None)));
            }
            manager
        });

        let shared = Rc::new(Shared {
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
        egui_ctx.set_visuals(match settings.theme {
            nrb_profile::Theme::Dark => egui::Visuals::dark(),
            nrb_profile::Theme::Light => egui::Visuals::light(),
        });

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
                bookmarked: false,
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
            modals: Default::default(),
            fullscreen: false,
            favicons: Default::default(),
            store,
            settings,
            profile_dir,
            library: Library::default(),
            nodes: config.nodes,
            nodes_window: NodesWindow::default(),
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

    fn content_size(&self) -> PhysicalSize<u32> {
        let size = self.window.inner_size();
        PhysicalSize::new(
            size.width.max(1),
            size.height.saturating_sub(self.toolbar_px).max(1),
        )
    }

    fn build_tab(&self, url: Url) -> WebView {
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
        self.window.request_redraw();
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
            self.toolbar.url_input = if url == "about:blank" {
                String::new()
            } else {
                url
            };
        }
    }

    fn spin(&mut self) {
        self.servo.spin_event_loop();
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
        let titled: Vec<WebViewId> = self.shared.titled.borrow_mut().drain(..).collect();
        if !self.settings.record_history || self.profile_dir.is_none() {
            return;
        }
        for id in completed {
            if let Some(tab) = self.tabs.iter().find(|tab| tab.id() == id)
                && let Some(url) = tab.url()
            {
                let title = tab.page_title().unwrap_or_default();
                let _ = self.store.record_visit(url.as_str(), &title);
            }
        }
        for id in titled {
            if let Some(tab) = self.tabs.iter().find(|tab| tab.id() == id)
                && let (Some(url), Some(title)) = (tab.url(), tab.page_title())
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
                        url: url.to_string(),
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

    fn apply_settings(&mut self) {
        self.search = self.settings.search.clone();
        self.shared.adblock_on.set(self.settings.adblock);
        self.egui_ctx.set_visuals(match self.settings.theme {
            nrb_profile::Theme::Dark => egui::Visuals::dark(),
            nrb_profile::Theme::Light => egui::Visuals::light(),
        });
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
        self.window.request_redraw();
    }

    fn set_fullscreen(&mut self, fullscreen: bool) {
        self.fullscreen = fullscreen;
        self.window
            .set_fullscreen(fullscreen.then_some(winit::window::Fullscreen::Borderless(None)));
        self.window.request_redraw();
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
        self.window.request_redraw();
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
            Action::ToggleBookmark => self.toggle_bookmark(),
            Action::OpenHistory => self.library.toggle(Panel::History),
            Action::OpenBookmarks => self.library.toggle(Panel::Bookmarks),
            Action::OpenSettings => self.library.toggle(Panel::Settings),
            Action::OpenNodes => self.nodes_window.toggle(),
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
            WKey::Named(WNamedKey::ArrowLeft) if alt => self.apply(Action::Back),
            WKey::Named(WNamedKey::ArrowRight) if alt => self.apply(Action::Forward),
            WKey::Named(WNamedKey::F5) => self.apply(Action::Reload),
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
            _ if ctrl && (character == "=" || character == "+") => self.apply(Action::ZoomIn),
            _ if ctrl && (character == "-" || character == "_") => self.apply(Action::ZoomOut),
            _ if ctrl && character == "0" => self.apply(Action::ZoomReset),
            _ if ctrl && character == "t" => self.apply(Action::NewTab),
            _ if ctrl && character == "w" => self.apply(Action::CloseTab(self.active)),
            _ if ctrl && character == "l" => self.toolbar.focus_url = true,
            _ if ctrl && character == "r" => self.apply(Action::Reload),
            _ => return false,
        }
        self.window.request_redraw();
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
        if response.repaint {
            self.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => {
                self.save_session(true);
                event_loop.exit();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Resized(size) => {
                self.window_ctx.resize(size);
                let content = self.content_size();
                self.content_ctx.resize(content);
                for tab in &self.tabs {
                    tab.resize(content);
                }
                self.window.request_redraw();
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
            WindowEvent::MouseWheel { delta, .. } => {
                if self.over_content()
                    && let Some(tab) = self.active_tab()
                {
                    let (x, y, mode) = match delta {
                        MouseScrollDelta::LineDelta(dx, dy) => (
                            f64::from(dx) * 76.0,
                            f64::from(dy) * 76.0,
                            WheelMode::DeltaLine,
                        ),
                        MouseScrollDelta::PixelDelta(delta) => {
                            (delta.x, delta.y, WheelMode::DeltaPixel)
                        }
                    };
                    tab.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                        WheelDelta { x, y, z: 0.0, mode },
                        self.content_point(),
                    )));
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
            WindowEvent::RedrawRequested => self.redraw(),
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

    fn redraw(&mut self) {
        use egui_glow::glow::HasContext as _;
        self.window_ctx.prepare_for_rendering();
        let size = self.window.inner_size();
        let scale = self.window.scale_factor() as f32;

        self.toolbar.tunnel = self.tunnel.as_ref().map(|t| (t.summary(), t.up()));
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
        if let Some(tab) = self.tabs.get(self.active) {
            tab.paint();
            self.blit_content();
            self.window.set_cursor(cursor_icon(tab.cursor()));
        }

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let mut actions = Vec::new();
        let mut content_top = 0.0_f32;
        let toolbar = &mut self.toolbar;
        let fullscreen = self.fullscreen;
        let mut modal_done = false;
        let mut library_actions = Vec::new();
        let mut nodes_restart = false;
        let mut output = self.egui_ctx.clone().run_ui(raw_input, |ui| {
            if !fullscreen {
                actions = ui::toolbar(ui, &tabs, toolbar);
                content_top = ui.available_rect_before_wrap().top();
            }
            library_actions = self.library.show(ui.ctx(), &self.store, &mut self.settings);
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
            }
        }
        if nodes_restart && let Some(manager) = self.nodes.clone() {
            self.save_session(true);
            manager.request_restart();
            self.exit = true;
        }
        if modal_done {
            self.modals.pop_front();
            self.window.request_redraw();
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

        // Фактическая высота тулбара (в пикселях) → размер области страницы.
        let toolbar_px = if self.fullscreen {
            0
        } else {
            (content_top * scale).round().max(1.0) as u32
        };
        if toolbar_px != self.toolbar_px {
            self.toolbar_px = toolbar_px;
            self.shared.toolbar_px.set(toolbar_px);
            let content = self.content_size();
            self.content_ctx.resize(content);
            for tab in &self.tabs {
                tab.resize(content);
            }
        }

        self.window_ctx.present();

        for action in actions {
            self.apply(action);
        }
        if self.egui_ctx.has_requested_repaint()
            || self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.animating())
        {
            self.window.request_redraw();
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
