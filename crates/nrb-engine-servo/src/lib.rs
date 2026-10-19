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

/// Встроенный скрипт совместимости с сайтами (`document.all`) — внедряется и в окно, и в `snap`.
pub const COMPAT_JS: &str = include_str!("compat.js");

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
    /// Все исходящие TCP-соединения движка идут через этот коннектор (N2 форка): Servo не открывает сокетов сам, настройки
    /// прокси `proxy` при этом не действуют. Нужен форк `nrxp-servo`.
    pub network_connector: Option<ConnectorHandle>,
    /// Локальный HTTP-прокси туннеля для WebRTC (TURN по TCP идёт через него, значит, через туннель). `None` — без туннеля.
    pub webrtc_proxy: Option<WebRtcProxy>,
    /// WebRTC запрещён совсем (герметичный режим: сокеты libnice вне нашего контроля, прокси туннеля из пустого
    /// пространства имён недоступен).
    pub webrtc_forbidden: bool,
}

/// Адрес и учётные данные локального прокси для ICE/TURN.
#[derive(Clone, Debug)]
pub struct WebRtcProxy {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

/// Ставит сетевую политику WebRTC движка: только relay (TURN по TCP), без UDP, без host/STUN-кандидатов и UPnP; TURN идёт
/// через прокси туннеля. Прямой режим WebRTC в браузере не предусмотрен. Без фичи `media` WebRTC в движке нет.
#[cfg(feature = "media")]
pub fn install_webrtc_policy(config: &ServoConfig) {
    servo::set_webrtc_network_policy(servo::WebRtcNetworkPolicy {
        relay_only: true,
        proxy: config
            .webrtc_proxy
            .as_ref()
            .map(|p| (p.host.clone(), p.port)),
        proxy_credentials: config
            .webrtc_proxy
            .as_ref()
            .map(|p| (p.username.clone(), p.password.clone())),
    });
}

#[cfg(not(feature = "media"))]
pub fn install_webrtc_policy(_config: &ServoConfig) {}

/// Обёртка над `Arc<dyn NetworkConnector>`: нужна, чтобы `ServoConfig` оставался `Debug + Clone`.
#[derive(Clone)]
pub struct ConnectorHandle(pub std::sync::Arc<dyn servo::NetworkConnector>);

impl std::fmt::Debug for ConnectorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectorHandle(..)")
    }
}

/// Применяет к `ServoBuilder` сетевые настройки из конфигурации (коннектор встраивателя).
pub fn apply_network(builder: servo::ServoBuilder, config: &ServoConfig) -> servo::ServoBuilder {
    match &config.network_connector {
        Some(handle) => builder.network_connector(handle.0.clone()),
        None => builder,
    }
}

pub use servo;

mod connector;
mod resources;
pub use connector::TransportConnector;
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
            "mse" => Some(&mut prefs.dom_mse_enabled),
            "webrtc" => Some(&mut prefs.dom_webrtc_enabled),
            "webrtc_transceiver" => Some(&mut prefs.dom_webrtc_transceiver_enabled),
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
        // Media Source Extensions (форк: fMP4 H.264/AAC поверх GStreamer) — только там, где есть воспроизведение.
        dom_mse_enabled: cfg!(feature = "media"),
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
        // Без профиля каталог для временных данных задаём сами: если оставить `None`, Servo создаёт временный каталог, не
        // удерживая его (`let _ = temp_dir;` ничего не захватывает) — тот удаляется сразу, хранилище IndexedDB падает на
        // «unable to open database file» и откатывается на базу в памяти с путями относительно текущего каталога
        // (`./bottles/…` — мусор в каталоге запуска).
        config_dir: config.profile_dir.clone().or_else(|| ephemeral_dir().ok()),
        temporary_storage: config.profile_dir.is_none(),
        // Мультипроцесс (фича `multiprocess`): `NRB_MULTIPROCESS=1`, песочница контент-процессов — `NRB_SANDBOX=1`.
        multiprocess: multiprocess_enabled(),
        sandbox: multiprocess_enabled() && sandbox_requested(),
        ..servo::Opts::default()
    }
}

/// Страницы работают в отдельных процессах (фича `multiprocess` + `NRB_MULTIPROCESS=1`).
///
/// Что от этого зависит: статики процесса движка не доходят до контент-процессов — в частности политика WebRTC
/// (`install_webrtc_policy`) там осталась бы по умолчанию (прямой ICE, утечка адреса), поэтому WebRTC в этом режиме
/// запрещён (`app.rs: apply_engine_policy`, `gui.rs`). Настройки-«prefs» контент-процесс получает снимком при запуске:
/// живое переключение действует только на новые процессы.
pub fn multiprocess_enabled() -> bool {
    cfg!(feature = "multiprocess") && env_flag("NRB_MULTIPROCESS")
}

/// Песочница контент-процессов Servo (gaol: seccomp + пространства имён; только Linux x86_64 и macOS).
///
/// Проверено 2026-10-12 на Linux: контент-процесс в песочнице молча убивается ядром (страница пустая, до таймаута), потому что
/// профиль gaol пускает его лишь к `/dev/urandom` и файлам ресурсов, а фильтр syscall'ов не знает того, что нужно движку
/// (шрифты, GStreamer, потоки SpiderMonkey). Поэтому `NRB_SANDBOX=1` не включает её, а предупреждает; включить на свой риск
/// для экспериментов — `NRB_SANDBOX=force`.
fn sandbox_requested() -> bool {
    match std::env::var("NRB_SANDBOX").as_deref() {
        Ok("force") => cfg!(any(
            target_os = "macos",
            all(target_os = "linux", target_arch = "x86_64")
        )),
        Ok("1") => {
            eprintln!(
                "NRB_SANDBOX=1: песочница контент-процессов Servo (gaol) пока убивает процесс страницы — не включена \
                 (NRB_SANDBOX=force включает её для экспериментов)"
            );
            false
        }
        _ => false,
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| v == "1")
}

/// Точка входа контент-процесса: Servo запускает его как `<наш бинарь> --content-process <токен>`. Возвращает `true`, если
/// процесс был контентным и уже отработал (вызывающий должен сразу выйти). Звать первым делом в `main`.
pub fn run_content_process_if_requested() -> bool {
    #[cfg(feature = "multiprocess")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(at) = args.iter().position(|a| a == "--content-process")
            && let Some(token) = args.get(at + 1)
        {
            servo::run_content_process(token.clone());
            return true;
        }
    }
    false
}

/// Каталог временных данных процесса: `<tmp>/nrb-ephemeral-<pid>`, права 0700. Убирается [`cleanup_ephemeral`];
/// каталоги умерших процессов подчищаются при создании.
pub fn ephemeral_dir() -> std::io::Result<std::path::PathBuf> {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    if let Some(dir) = DIR.get() {
        return Ok(dir.clone());
    }
    let base = std::env::temp_dir();
    sweep_stale_ephemeral(&base);
    let dir = base.join(format!("nrb-ephemeral-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(DIR.get_or_init(|| dir).clone())
}

/// Удаляет временный каталог этого процесса (звать перед выходом; безопасно звать без него).
pub fn cleanup_ephemeral() {
    let dir = std::env::temp_dir().join(format!("nrb-ephemeral-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(dir);
}

/// Убирает `nrb-ephemeral-<pid>` процессов, которых уже нет (после аварийного выхода).
fn sweep_stale_ephemeral(base: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("nrb-ephemeral-"))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() && cfg!(target_os = "linux") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
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

/// Заголовок страницы-проверки антибота: она сама перенаправит, когда проверка закончится (см. `snapshot`).
fn is_interstitial_title(title: &str) -> bool {
    let title = title.to_lowercase();
    [
        "just a moment",
        "checking your browser",
        "checking if the site connection is secure",
        "pardon our interruption",
        "attention required",
        "verifying you are human",
        "one moment, please",
        "ddos-guard",
        "проверка браузера",
        "проверяем ваш браузер",
        "подождите",
    ]
    .iter()
    .any(|marker| title.contains(marker))
}

pub struct ServoSnapshotter {
    servo: Servo,
    state: Rc<PageState>,
    user_content: Option<Rc<servo::UserContentManager>>,
}

impl ServoSnapshotter {
    pub fn new(config: &ServoConfig) -> anyhow::Result<Self> {
        install_crypto_provider();

        let servo = apply_network(
            ServoBuilder::default()
                .opts(build_opts(config))
                .preferences(build_preferences(config))
                .event_loop_waker(Box::new(PollWaker(Arc::new(AtomicBool::new(false))))),
            config,
        )
        .build();
        let state = Rc::new(PageState::default());
        servo.set_delegate(Rc::new(ConsoleDelegate(state.clone())));
        let user_content = Some({
            let manager = Rc::new(servo::UserContentManager::new(&servo));
            manager.add_script(Rc::new(servo::UserScript::new(COMPAT_JS.to_owned(), None)));
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
        let mut completed = self.pump(Some(&webview), deadline, || {
            state.status.get() == Some(LoadStatus::Complete)
        })?;
        let load_time = started.elapsed();

        // Страницы, дорисовывающиеся скриптами: даём движку поработать ещё `settle`. Если за это время страница ушла дальше
        // (скриптовые редиректы: SSO Яндекса — цепочка страниц с `location.replace`/`form.submit()`, у каждой свой `Complete`),
        // ждём `Complete` уже следующей страницы, а не снимаем промежуточную. Раундов немного: цепочки бесконечными бывают.
        if !req.settle.is_zero() {
            for _ in 0..10 {
                let url_before = webview.url();
                self.pump(Some(&webview), Instant::now() + req.settle, || false)?;
                let moved = webview.url() != url_before
                    || self.state.status.get() != Some(LoadStatus::Complete);
                if !moved || Instant::now() >= deadline {
                    break;
                }
                let state = self.state.clone();
                completed = self.pump(Some(&webview), deadline, || {
                    state.status.get() == Some(LoadStatus::Complete)
                })?;
                if !completed {
                    break;
                }
            }
        }

        // Страницы-проверки («Checking your browser…», «Just a moment…», «Pardon Our Interruption…») сами перенаправляют на
        // нужную страницу через несколько секунд, когда скрипт проверки закончит работу. Снимать их бессмысленно: ждём, пока
        // заголовок/адрес не сменятся, пока не выйдет общее время.
        let mut challenge_rounds = 0;
        while webview
            .page_title()
            .as_deref()
            .is_some_and(is_interstitial_title)
            && challenge_rounds < 15
            && Instant::now() + Duration::from_secs(3) < deadline
        {
            challenge_rounds += 1;
            self.pump(
                Some(&webview),
                Instant::now() + Duration::from_secs(2),
                || false,
            )?;
        }

        // Штатный скриншот ждёт «полной готовности» (все картинки, шрифты, отсутствие блокирующих
        // ресурсов). У живых сайтов с фоновыми запросами или упавшим скриптом она не наступает —
        // тогда берём то, что уже нарисовано, и помечаем снимок как неполный.
        let mut partial = !completed;
        let mut image = None;
        // Пустой кадр у страницы после цепочки скриптовых редиректов (SSO Яндекса: на каждом шаге несколько секунд сети) —
        // чаще «ещё не дошли», чем «пусто»: даём до трёх дополнительных раундов по 2 с, пока кадр не перестанет быть пустым.
        for round in 0..4 {
            if !completed {
                break;
            }
            if round > 0 {
                if Instant::now() + Duration::from_secs(4) >= deadline {
                    break;
                }
                self.pump(
                    Some(&webview),
                    Instant::now() + Duration::from_secs(2),
                    || false,
                )?;
            }
            let sink = self.state.clone();
            webview.take_screenshot(None, move |result| {
                *sink.screenshot.borrow_mut() = Some(result.map_err(|e| format!("{e:?}")));
            });
            let state = self.state.clone();
            let shot_deadline = Instant::now() + req.timeout.min(Duration::from_secs(15));
            image = None;
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
            let blank = image.as_ref().is_some_and(|img| {
                let (w, h) = img.dimensions();
                Frame {
                    size: Size {
                        width: w,
                        height: h,
                    },
                    rgba: img.as_raw().clone(),
                }
                .dominant_color_ratio()
                    >= 0.999
            });
            if !blank {
                break;
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

#[cfg(test)]
mod interstitial_tests {
    use super::is_interstitial_title;

    #[test]
    fn challenge_titles_are_recognised() {
        for title in [
            "Just a moment...",
            "Pardon Our Interruption...",
            "Attention Required! | Cloudflare",
            "Проверка браузера перед переходом на сайт",
        ] {
            assert!(is_interstitial_title(title), "{title}");
        }
        for title in [
            "Rust Programming Language",
            "eBay",
            "Newest Questions - Stack Overflow",
            "",
        ] {
            assert!(!is_interstitial_title(title), "{title}");
        }
    }
}
