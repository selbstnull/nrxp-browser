//! Менеджер паролей в оболочке: панель «Сохранить пароль?», окно «Пароли», запрос мастер-пароля, подстановка по команде.
//! Хранилище и криптография — крейт `nrb-vault`; страница отдаёт введённое через `pwm.js`, оболочка опрашивает её раз в
//! 1,5 с вместе с опросом звука. Разблокировка Argon2id (~0,5 с) идёт в потоке интерфейса: один раз за сеанс.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use nrb_engine_servo::servo::WebViewId;
use nrb_vault::{Entry, Error as VaultError, KdfParams, SaveOutcome, Vault};
use url::Url;

/// Введённое пользователем на странице, пойманное `pwm.js`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingLogin {
    pub origin: String,
    pub username: String,
    pub password: String,
}

/// Результат разбора ответа `__nrbPw.poll()`.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct PollResult {
    pub login_form: bool,
    pub pending: Option<PendingLogin>,
}

pub const POLL_SCRIPT: &str = "window.__nrbPw ? window.__nrbPw.poll() : ''";

pub fn parse_poll(json: &str) -> Option<PollResult> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let pending = value.get("pending").and_then(|p| {
        let get = |key: &str| p.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        Some(PendingLogin {
            origin: get("origin")?,
            username: get("username").unwrap_or_default(),
            password: get("password")?,
        })
    });
    Some(PollResult {
        login_form: value
            .get("login")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        pending,
    })
}

/// Скрипт подстановки (значения экранируются JSON-литералами).
pub fn fill_script(username: &str, password: &str) -> String {
    format!(
        "window.__nrbPw ? window.__nrbPw.fill({}, {}) : false",
        serde_json::to_string(username).unwrap_or_else(|_| "\"\"".into()),
        serde_json::to_string(password).unwrap_or_else(|_| "\"\"".into())
    )
}

/// Источник страницы для хранилища: только http/https, `схема://хост[:порт]`.
pub fn origin_of(url: &Url) -> Option<String> {
    matches!(url.scheme(), "http" | "https")
        .then(|| url.origin().ascii_serialization())
        .filter(|o| o != "null")
}

/// Что оболочке сделать по итогам кадра интерфейса.
pub enum Effect {
    /// Подставить логин/пароль в страницу вкладки `tab`.
    Fill {
        tab: usize,
        username: String,
        password: String,
    },
    Notice(String),
}

enum Purpose {
    /// Сохранить введённое (панель нажали при запертом хранилище).
    Save(PendingLogin),
    Never(String),
    Fill,
    OpenWindow,
}

struct UnlockDialog {
    purpose: Purpose,
    create: bool,
    master: String,
    repeat: String,
    error: Option<String>,
    focus: bool,
}

struct SavePrompt {
    login: PendingLogin,
    /// Известно только при разблокированном хранилище.
    outcome: Option<SaveOutcome>,
}

pub struct Passwords {
    vault: Option<Vault>,
    pub enabled: bool,
    prompt: Option<SavePrompt>,
    unlock: Option<UnlockDialog>,
    window_open: bool,
    /// Запись, пароль которой показан в окне, и когда скрыть.
    revealed: Option<(u64, Instant)>,
    chooser: Option<(usize, Vec<Entry>)>,
    /// Уже отклонённые в этом сеансе пары (источник, логин): не надоедаем одним и тем же.
    dismissed: HashSet<(String, String)>,
    /// Вкладки, на которых сейчас видна форма с паролем (для подсказки в тулбаре).
    pub login_tabs: HashSet<WebViewId>,
    /// Сырые ответы опроса из колбэков Servo: (идентификатор вкладки, JSON).
    pub inbox: Rc<RefCell<Vec<(WebViewId, String)>>>,
    queue: VecDeque<PendingLogin>,
    pending_fill_tab: Option<usize>,
    change_master: Option<(String, String)>,
    notice: Option<(String, Instant)>,
}

impl Passwords {
    /// `profile_dir` — каталог профиля; `None` (приватный сеанс) отключает менеджер.
    pub fn new(profile_dir: Option<&Path>, enabled: bool) -> Self {
        Self {
            vault: profile_dir.map(Vault::open),
            enabled: enabled && profile_dir.is_some(),
            prompt: None,
            unlock: None,
            window_open: false,
            revealed: None,
            chooser: None,
            dismissed: HashSet::new(),
            login_tabs: HashSet::new(),
            inbox: Rc::default(),
            queue: VecDeque::new(),
            pending_fill_tab: None,
            change_master: None,
            notice: None,
        }
    }

    pub fn available(&self) -> bool {
        self.enabled && self.vault.is_some()
    }

    pub fn toggle_window(&mut self) {
        if !self.available() {
            return;
        }
        if self.window_open {
            self.window_open = false;
        } else if self.vault.as_ref().is_some_and(Vault::is_unlocked) {
            self.window_open = true;
        } else {
            self.open_unlock(Purpose::OpenWindow);
        }
    }

    /// Запросить подстановку для вкладки `tab` с адресом `origin`.
    pub fn request_fill(&mut self, tab: usize, origin: Option<&str>) -> Vec<Effect> {
        let Some(origin) = origin else {
            return vec![Effect::Notice(
                "Пароли работают только на http/https-страницах".into(),
            )];
        };
        if !self.available() {
            return vec![Effect::Notice(
                "Менеджер паролей выключен (приватный сеанс или настройки)".into(),
            )];
        }
        if !self.vault.as_ref().is_some_and(Vault::is_unlocked) {
            self.pending_fill_tab = Some(tab);
            self.open_unlock(Purpose::Fill);
            return Vec::new();
        }
        self.fill_from_vault(tab, origin)
    }

    fn fill_from_vault(&mut self, tab: usize, origin: &str) -> Vec<Effect> {
        let entries = self
            .vault
            .as_ref()
            .and_then(|v| v.logins_for(origin).ok())
            .unwrap_or_default();
        match entries.len() {
            0 => vec![Effect::Notice(format!(
                "Для {origin} нет сохранённых паролей"
            ))],
            1 => {
                let entry = &entries[0];
                vec![Effect::Fill {
                    tab,
                    username: entry.username.clone(),
                    password: entry.password.clone(),
                }]
            }
            _ => {
                self.chooser = Some((tab, entries));
                Vec::new()
            }
        }
    }

    /// Разобрать ответ опроса вкладки: форма на странице и введённое пользователем.
    pub fn ingest(&mut self, tab_id: WebViewId, json: &str) {
        let Some(poll) = parse_poll(json) else {
            return;
        };
        if poll.login_form {
            self.login_tabs.insert(tab_id);
        } else {
            self.login_tabs.remove(&tab_id);
        }
        if let Some(login) = poll
            .pending
            .filter(|_| self.available() && self.vault.is_some())
        {
            self.on_pending(login);
        }
    }

    fn on_pending(&mut self, login: PendingLogin) {
        if login.password.is_empty()
            || self
                .dismissed
                .contains(&(login.origin.clone(), login.username.clone()))
        {
            return;
        }
        let mut outcome = None;
        if let Some(vault) = self.vault.as_ref().filter(|v| v.is_unlocked()) {
            if vault.never_save_for(&login.origin).unwrap_or(false) {
                return;
            }
            outcome = vault
                .preview_save(&login.origin, &login.username, &login.password)
                .ok();
            if outcome == Some(SaveOutcome::Unchanged) {
                return;
            }
        }
        // Новая панель заменяет прежнюю (например, пользователь исправил пароль и отправил снова).
        self.prompt = Some(SavePrompt { login, outcome });
    }

    fn open_unlock(&mut self, purpose: Purpose) {
        let create = self.vault.as_ref().is_some_and(|v| !v.exists());
        self.unlock = Some(UnlockDialog {
            purpose,
            create,
            master: String::new(),
            repeat: String::new(),
            error: None,
            focus: true,
        });
    }

    fn finish_purpose(&mut self, purpose: Purpose, origin_for_fill: Option<String>) -> Vec<Effect> {
        let mut effects = Vec::new();
        match purpose {
            Purpose::Save(login) => effects.extend(self.commit_save(login)),
            Purpose::Never(origin) => {
                if let Some(vault) = self.vault.as_mut() {
                    let _ = vault.set_never_save(&origin, true);
                }
                self.prompt = None;
                effects.push(Effect::Notice(format!(
                    "Для {origin} пароли предлагать не будем"
                )));
            }
            Purpose::Fill => {
                if let (Some(tab), Some(origin)) = (self.pending_fill_tab.take(), origin_for_fill) {
                    effects.extend(self.fill_from_vault(tab, &origin));
                }
            }
            Purpose::OpenWindow => self.window_open = true,
        }
        effects
    }

    fn commit_save(&mut self, login: PendingLogin) -> Option<Effect> {
        let vault = self.vault.as_mut()?;
        self.prompt = None;
        match vault.save_login(&login.origin, &login.username, &login.password) {
            Ok(SaveOutcome::Added) => Some(Effect::Notice("Пароль сохранён".into())),
            Ok(SaveOutcome::Updated) => Some(Effect::Notice("Пароль обновлён".into())),
            Ok(SaveOutcome::Unchanged) => None,
            Err(error) => Some(Effect::Notice(format!("Не удалось сохранить: {error}"))),
        }
    }

    /// Панель «Сохранить пароль?» (под тулбаром) и всплывающие окна. `origin_of_active` — источник активной вкладки.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        active_tab: usize,
        origin_of_active: Option<String>,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some((_, until)) = &self.notice
            && Instant::now() >= *until
        {
            self.notice = None;
        }
        if let Some((_, until)) = self.revealed
            && Instant::now() >= until
        {
            self.revealed = None;
        }
        if !self.available() {
            return effects;
        }

        // --- панель «Сохранить пароль?»
        if let Some(prompt) = self.prompt.take() {
            let mut keep = true;
            let mut action: Option<&'static str> = None;
            ui.horizontal(|ui| {
                let who = if prompt.login.username.is_empty() {
                    String::new()
                } else {
                    format!(" для «{}»", prompt.login.username)
                };
                let verb = match prompt.outcome {
                    Some(SaveOutcome::Updated) => "Обновить сохранённый пароль",
                    _ => "Сохранить пароль",
                };
                ui.label(format!("{verb}{who} на {}?", prompt.login.origin));
                if ui.button("Сохранить").clicked() {
                    action = Some("save");
                }
                if ui.button("Не сейчас").clicked() {
                    action = Some("later");
                }
                if ui.button("Никогда для этого сайта").clicked() {
                    action = Some("never");
                }
            });
            match action {
                Some("save") => {
                    keep = false;
                    if self.vault.as_ref().is_some_and(Vault::is_unlocked) {
                        effects.extend(self.commit_save(prompt.login.clone()));
                    } else {
                        self.open_unlock(Purpose::Save(prompt.login.clone()));
                    }
                }
                Some("later") => {
                    keep = false;
                    self.dismissed
                        .insert((prompt.login.origin.clone(), prompt.login.username.clone()));
                }
                Some("never") => {
                    keep = false;
                    if self.vault.as_ref().is_some_and(Vault::is_unlocked) {
                        effects.extend(
                            self.finish_purpose(Purpose::Never(prompt.login.origin.clone()), None),
                        );
                    } else {
                        self.open_unlock(Purpose::Never(prompt.login.origin.clone()));
                    }
                }
                _ => {}
            }
            if keep {
                self.prompt = Some(prompt);
            }
        }

        let ctx = ui.ctx().clone();
        // --- мастер-пароль
        if self.unlock.is_some() {
            effects.extend(self.show_unlock(&ctx, origin_of_active.clone()));
        }
        // --- выбор записи при нескольких логинах
        if let Some((tab, entries)) = self.chooser.take() {
            let mut keep = true;
            egui::Window::new("Выберите вход")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
                .show(&ctx, |ui| {
                    for entry in &entries {
                        let label = if entry.username.is_empty() {
                            "(без логина)".to_owned()
                        } else {
                            entry.username.clone()
                        };
                        if ui.button(label).clicked() {
                            effects.push(Effect::Fill {
                                tab,
                                username: entry.username.clone(),
                                password: entry.password.clone(),
                            });
                            keep = false;
                        }
                    }
                    if ui.button("Отмена").clicked() {
                        keep = false;
                    }
                });
            if keep {
                self.chooser = Some((tab, entries));
            }
        }
        // --- окно «Пароли»
        if self.window_open {
            effects.extend(self.show_window(&ctx));
        }
        let _ = active_tab;
        effects
    }

    fn show_unlock(&mut self, ctx: &egui::Context, origin_for_fill: Option<String>) -> Vec<Effect> {
        let Some(mut dialog) = self.unlock.take() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        let mut submit = false;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Window::new(if dialog.create {
            "Создать хранилище паролей"
        } else {
            "Мастер-пароль"
        })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
        .show(ctx, |ui| {
            if dialog.create {
                ui.label(
                    "Придумайте мастер-пароль (от 8 символов). Забыв его, пароли не восстановить.",
                );
            }
            let first = ui.add(
                egui::TextEdit::singleline(&mut dialog.master)
                    .password(true)
                    .hint_text("мастер-пароль"),
            );
            if dialog.focus {
                first.request_focus();
                dialog.focus = false;
            }
            if dialog.create {
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.repeat)
                        .password(true)
                        .hint_text("повторите"),
                );
            }
            if let Some(error) = &dialog.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            ui.horizontal(|ui| {
                if ui
                    .button(if dialog.create {
                        "Создать"
                    } else {
                        "Открыть"
                    })
                    .clicked()
                    || (first.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    submit = true;
                }
                if ui.button("Отмена").clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.pending_fill_tab = None;
            return effects;
        }
        if submit {
            let result = (|| -> Result<(), VaultError> {
                let vault = self.vault.as_mut().ok_or(VaultError::NoVault)?;
                if dialog.create {
                    if dialog.master != dialog.repeat {
                        return Err(VaultError::Crypto("пароли не совпадают".into()));
                    }
                    vault.create(&dialog.master, KdfParams::default())
                } else {
                    vault.unlock(&dialog.master)
                }
            })();
            match result {
                Ok(()) => {
                    dialog.master.clear();
                    dialog.repeat.clear();
                    effects.extend(self.finish_purpose(dialog.purpose, origin_for_fill));
                    return effects;
                }
                Err(error) => {
                    dialog.error = Some(error.to_string());
                    dialog.master.clear();
                    dialog.repeat.clear();
                    dialog.focus = true;
                }
            }
        }
        self.unlock = Some(dialog);
        effects
    }

    fn show_window(&mut self, ctx: &egui::Context) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut open = true;
        let entries = self
            .vault
            .as_ref()
            .and_then(|v| v.all().ok())
            .unwrap_or_default();
        let mut delete: Option<u64> = None;
        let mut reveal: Option<u64> = None;
        let mut lock = false;
        egui::Window::new("Пароли")
            .open(&mut open)
            .default_size([560.0, 380.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Заблокировать").clicked() {
                        lock = true;
                    }
                    ui.weak(format!("записей: {}", entries.len()));
                });
                ui.collapsing("Сменить мастер-пароль", |ui| {
                    let (old, new) = self.change_master.get_or_insert_with(Default::default);
                    ui.add(
                        egui::TextEdit::singleline(old)
                            .password(true)
                            .hint_text("текущий"),
                    );
                    ui.add(
                        egui::TextEdit::singleline(new)
                            .password(true)
                            .hint_text("новый (от 8 символов)"),
                    );
                    if ui.button("Сменить").clicked() {
                        let (old, new) = self.change_master.take().unwrap_or_default();
                        let text = match self.vault.as_mut().map(|v| v.change_master(&old, &new)) {
                            Some(Ok(())) => "Мастер-пароль изменён".to_owned(),
                            Some(Err(error)) => format!("Не удалось: {error}"),
                            None => "Хранилища нет".to_owned(),
                        };
                        effects.push(Effect::Notice(text));
                    }
                });
                ui.separator();
                if entries.is_empty() {
                    ui.weak("Пока пусто: после входа на сайте браузер предложит сохранить пароль.");
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for entry in &entries {
                        ui.horizontal(|ui| {
                            ui.label(&entry.origin);
                            ui.strong(if entry.username.is_empty() {
                                "(без логина)"
                            } else {
                                &entry.username
                            });
                            let shown = self.revealed.is_some_and(|(id, _)| id == entry.id);
                            if shown {
                                ui.monospace(&entry.password);
                            }
                            if ui
                                .small_button(if shown {
                                    "Скрыть"
                                } else {
                                    "Показать"
                                })
                                .clicked()
                            {
                                reveal = Some(entry.id);
                            }
                            if ui.small_button("Копировать").clicked() {
                                ui.ctx().copy_text(entry.password.clone());
                                effects.push(Effect::Notice(
                                    "Пароль скопирован в буфер обмена".into(),
                                ));
                            }
                            if ui.small_button("Удалить").clicked() {
                                delete = Some(entry.id);
                            }
                        });
                    }
                });
            });
        if let Some(id) = reveal {
            self.revealed = match self.revealed {
                Some((shown, _)) if shown == id => None,
                _ => Some((id, Instant::now() + Duration::from_secs(10))),
            };
        }
        if let Some(id) = delete
            && let Some(vault) = self.vault.as_mut()
        {
            let _ = vault.delete(id);
        }
        if lock {
            if let Some(vault) = self.vault.as_mut() {
                vault.lock();
            }
            self.window_open = false;
            self.revealed = None;
        }
        if !open {
            self.window_open = false;
            self.revealed = None;
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_results_are_parsed() {
        let parsed = parse_poll(r#"{"login":true,"pending":{"origin":"https://a.test","username":"ann","password":"p\"w"}}"#).unwrap();
        assert!(parsed.login_form);
        assert_eq!(
            parsed.pending,
            Some(PendingLogin {
                origin: "https://a.test".into(),
                username: "ann".into(),
                password: "p\"w".into()
            })
        );
        let none = parse_poll(r#"{"login":false,"pending":null}"#).unwrap();
        assert!(!none.login_form && none.pending.is_none());
        assert!(parse_poll("").is_none());
        assert!(parse_poll("мусор").is_none());
        // Нет пароля — это не вход.
        assert!(
            parse_poll(r#"{"login":true,"pending":{"origin":"https://a.test","username":"u"}}"#)
                .unwrap()
                .pending
                .is_none()
        );
    }

    #[test]
    fn fill_script_escapes_values() {
        let script = fill_script("a\"b", "</script>\\");
        assert!(script.contains(r#""a\"b""#), "{script}");
        assert!(script.contains(r#""</script>\\""#), "{script}");
    }

    #[test]
    fn origins_only_for_web_pages() {
        let o = |s: &str| origin_of(&Url::parse(s).unwrap());
        assert_eq!(
            o("https://a.test/path?q=1").as_deref(),
            Some("https://a.test")
        );
        assert_eq!(
            o("http://a.test:8080/").as_deref(),
            Some("http://a.test:8080")
        );
        assert_eq!(o("file:///tmp/x.html"), None);
        assert_eq!(o("about:blank"), None);
    }
}
