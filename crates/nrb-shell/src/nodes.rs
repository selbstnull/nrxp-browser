//! Окно «Узлы» (I7.1): спаривание с чужим узлом, выбор узла для туннеля, свой узел (создать, включить приём,
//! приглашения, устройства, отзыв). Всё, что связано с NRXP, делает `NodeManager` — его реализует `nrb`,
//! оболочка про узлы ничего не знает.

/// Спаренный узел (конфиг из `nrb pair`).
#[derive(Clone)]
pub struct PairedNode {
    pub name: String,
    pub addr: String,
    pub selected: bool,
}

#[derive(Clone)]
pub struct DeviceRow {
    pub name: String,
    pub id: String,
    pub egress: bool,
    pub revoked: bool,
    /// Выход клиента: `None` — по режиму узла, `Some("local")`, иначе метка домашнего выхода.
    pub exit_via: Option<String>,
}

/// Состояние собственного узла.
#[derive(Clone, Default)]
pub struct OwnNode {
    pub exists: bool,
    pub running: bool,
    pub listen: String,
    pub public_host: String,
    pub public_key: String,
    /// `local` / `prefer` / `require`.
    pub exit: String,
    pub devices: Vec<DeviceRow>,
    /// Подписи подключённых домашних выходов.
    pub live_egress: Vec<String>,
    pub sessions: usize,
}

/// Ошибки — строки для показа пользователю (без секретов).
pub trait NodeManager: Send + Sync {
    fn paired(&self) -> Vec<PairedNode>;
    /// Сохраняет конфиг из приглашения; возвращает имя узла.
    fn pair(&self, link: &str) -> Result<String, String>;
    /// Узел для туннеля (`None` — напрямую). Применяется после перезапуска.
    fn select(&self, name: Option<&str>) -> Result<(), String>;
    /// Узел задан в командной строке — выбор в окне не действует.
    fn cli_override(&self) -> bool;
    fn own(&self) -> OwnNode;
    fn init_own(&self, public_host: &str, port: u16) -> Result<(), String>;
    fn set_ingress(&self, on: bool) -> Result<(), String>;
    /// Ссылка-приглашение с секретами; токен показывается один раз.
    fn invite(&self, device: &str, egress: bool) -> Result<String, String>;
    fn revoke(&self, device: &str) -> Result<(), String>;
    /// Режим выхода клиентов узла: `local` | `prefer` | `require` (узел подхватывает изменение сам).
    fn set_exit_mode(&self, mode: &str) -> Result<(), String>;
    /// Выход одного клиентского устройства: `None` — по режиму узла, `Some("local")`, иначе метка выхода.
    fn set_device_exit(&self, device: &str, via: Option<&str>) -> Result<(), String>;
    /// Завершить окно и запустить `nrb` заново с теми же аргументами (после смены узла).
    fn request_restart(&self);
}

#[derive(Default)]
pub struct NodesWindow {
    pub open: bool,
    link: String,
    notice: Option<(bool, String)>,
    init_host: String,
    init_port: String,
    invite_name: String,
    invite_egress: bool,
    last_invite: Option<(String, bool)>,
    selection_dirty: bool,
}

fn short_key(key: &str) -> String {
    if key.len() > 16 {
        format!("{}…{}", &key[..8], &key[key.len() - 8..])
    } else {
        key.to_owned()
    }
}

impl NodesWindow {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    fn ok(&mut self, text: impl Into<String>) {
        self.notice = Some((true, text.into()));
    }

    fn err(&mut self, text: impl Into<String>) {
        self.notice = Some((false, text.into()));
    }

    /// Возвращает `true`, если пользователь попросил перезапуск.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        manager: &dyn NodeManager,
        tunnel_summary: Option<String>,
    ) -> bool {
        if !self.open {
            return false;
        }
        let mut keep_open = true;
        let mut restart = false;
        egui::Window::new("Узлы")
            .open(&mut keep_open)
            .default_size([640.0, 560.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.connection_section(ui, manager, tunnel_summary, &mut restart);
                    ui.add_space(8.0);
                    self.own_section(ui, manager);
                    if let Some((good, text)) = &self.notice {
                        ui.add_space(6.0);
                        let color = if *good {
                            egui::Color32::LIGHT_GREEN
                        } else {
                            egui::Color32::LIGHT_RED
                        };
                        ui.colored_label(color, text);
                    }
                });
            });
        if !keep_open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
        }
        restart
    }

    fn connection_section(
        &mut self,
        ui: &mut egui::Ui,
        manager: &dyn NodeManager,
        tunnel_summary: Option<String>,
        restart: &mut bool,
    ) {
        ui.heading("Подключение");
        match &tunnel_summary {
            Some(text) => ui.label(format!("Сейчас: {text}")),
            None => ui.label("Сейчас: напрямую, без узла"),
        };
        let paired = manager.paired();
        let override_cli = manager.cli_override();
        if override_cli {
            ui.weak("Узел задан в командной строке (--node) — выбор здесь не действует.");
        }
        ui.add_enabled_ui(!override_cli, |ui| {
            let mut selected: Option<Option<String>> = None;
            if ui
                .radio(
                    !paired.iter().any(|p| p.selected),
                    "Напрямую (без узла)",
                )
                .clicked()
            {
                selected = Some(None);
            }
            for node in &paired {
                if ui
                    .radio(node.selected, format!("{}  ·  {}", node.name, node.addr))
                    .clicked()
                {
                    selected = Some(Some(node.name.clone()));
                }
            }
            if let Some(choice) = selected {
                match manager.select(choice.as_deref()) {
                    Ok(()) => {
                        self.selection_dirty = true;
                        self.ok("Выбор сохранён. Чтобы страницы пошли через узел, перезапустите браузер.");
                    }
                    Err(error) => self.err(error),
                }
            }
        });
        if self.selection_dirty
            && ui
                .button("Перезапустить браузер с выбранным узлом")
                .clicked()
        {
            *restart = true;
        }
        ui.add_space(4.0);
        ui.label("Спариться с узлом по приглашению (nrxp://…):");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.link)
                    .hint_text("вставьте ссылку-приглашение")
                    .password(true)
                    .desired_width(380.0),
            );
            if ui.button("Спарить").clicked() {
                let link = std::mem::take(&mut self.link);
                match manager.pair(link.trim()) {
                    Ok(name) => self.ok(format!(
                        "Спарено с узлом «{name}». Выберите его выше и перезапустите браузер."
                    )),
                    Err(error) => self.err(error),
                }
            }
        });
        ui.weak("В приглашении секреты узла: оно скрыто при вводе и нигде не показывается.");
    }

    fn own_section(&mut self, ui: &mut egui::Ui, manager: &dyn NodeManager) {
        ui.heading("Свой узел");
        let own = manager.own();
        if !own.exists {
            ui.label(
                "Узла ещё нет. Адрес — внешний IP или домен, по которому к вам будут подключаться.",
            );
            ui.horizontal(|ui| {
                ui.label("Адрес");
                ui.add(
                    egui::TextEdit::singleline(&mut self.init_host)
                        .hint_text("203.0.113.5")
                        .desired_width(200.0),
                );
                ui.label("Порт");
                ui.add(
                    egui::TextEdit::singleline(&mut self.init_port)
                        .hint_text("443")
                        .desired_width(60.0),
                );
                if ui.button("Создать узел").clicked() {
                    let port = self.init_port.trim().parse::<u16>().unwrap_or(443);
                    match manager.init_own(self.init_host.trim(), port) {
                        Ok(()) => self.ok("Узел создан. Включите приём и выдайте приглашение."),
                        Err(error) => self.err(error),
                    }
                }
            });
            ui.weak(
                "Развернуть узел на сервере одной командой: `nrb deploy user@host --binary self`.",
            );
            return;
        }
        ui.label(format!(
            "Адрес в приглашениях: {}   ·   слушает: {}   ·   ключ: {}",
            own.public_host,
            own.listen,
            short_key(&own.public_key)
        ));
        ui.horizontal(|ui| {
            let label = if own.running {
                "Приём включён"
            } else {
                "Приём выключен"
            };
            ui.label(label);
            if ui
                .button(if own.running {
                    "Выключить"
                } else {
                    "Включить"
                })
                .clicked()
            {
                match manager.set_ingress(!own.running) {
                    Ok(()) => self.ok(if own.running {
                        "Приём выключен."
                    } else {
                        "Приём включён."
                    }),
                    Err(error) => self.err(error),
                }
            }
            if own.running {
                ui.weak(format!("сессий: {}", own.sessions));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Выход клиентов:");
            for (value, text) in [
                ("local", "с самого узла"),
                ("prefer", "через домашний выход, если подключён"),
                ("require", "ТОЛЬКО через домашний выход"),
            ] {
                if ui.radio(own.exit == value, text).clicked() && own.exit != value {
                    match manager.set_exit_mode(value) {
                        Ok(()) => self.ok("Режим выхода сохранён (узел подхватит его сам)."),
                        Err(error) => self.err(error),
                    }
                }
            }
        });
        ui.weak(if own.live_egress.is_empty() {
            "Домашних выходов не подключено (nrb egress '<ссылка>' на домашнем устройстве)."
                .to_owned()
        } else {
            format!("Подключён выход: {}", own.live_egress.join(", "))
        });

        ui.add_space(4.0);
        ui.label("Новое приглашение:");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.invite_name)
                    .hint_text("имя устройства")
                    .desired_width(200.0),
            );
            ui.checkbox(&mut self.invite_egress, "домашний выход (nrb egress)");
            if ui.button("Выдать").clicked() {
                match manager.invite(self.invite_name.trim(), self.invite_egress) {
                    Ok(link) => {
                        self.last_invite = Some((link, self.invite_egress));
                        self.invite_name.clear();
                        self.ok("Приглашение выдано. Токен показан один раз — скопируйте сейчас.");
                    }
                    Err(error) => self.err(error),
                }
            }
        });
        if let Some((link, egress)) = self.last_invite.clone() {
            ui.horizontal(|ui| {
                let mut shown = link.clone();
                ui.add(egui::TextEdit::singleline(&mut shown).desired_width(420.0));
                if ui.button("Копировать").clicked() {
                    ui.ctx().copy_text(link.clone());
                }
                if ui.button("Скрыть").clicked() {
                    self.last_invite = None;
                }
            });
            ui.colored_label(
                egui::Color32::from_rgb(250, 200, 50),
                "В ссылке секреты — передавайте только по доверенному каналу.",
            );
            if egress {
                ui.weak("На домашнем устройстве: nrb egress '<ссылка>'.");
            } else {
                ui.weak("На другом устройстве: «Спарить» в этом окне или nrb pair '<ссылка>'.");
            }
        }

        ui.add_space(4.0);
        ui.label("Устройства:");
        if own.devices.is_empty() {
            ui.weak("Пока нет.");
        }
        for device in &own.devices {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{}  ·  {}  ·  {}",
                    device.name,
                    if device.egress {
                        "выход"
                    } else {
                        "клиент"
                    },
                    device.id
                ));
                if !device.egress && !device.revoked {
                    let current = match device.exit_via.as_deref() {
                        None => "по режиму узла".to_owned(),
                        Some("local") => "с узла".to_owned(),
                        Some(label) => format!("через «{label}»"),
                    };
                    egui::ComboBox::from_id_salt(("exit", &device.id))
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            let mut choice: Option<Option<String>> = None;
                            if ui
                                .selectable_label(device.exit_via.is_none(), "по режиму узла")
                                .clicked()
                            {
                                choice = Some(None);
                            }
                            if ui
                                .selectable_label(
                                    device.exit_via.as_deref() == Some("local"),
                                    "всегда с узла",
                                )
                                .clicked()
                            {
                                choice = Some(Some("local".into()));
                            }
                            let mut labels = own.live_egress.clone();
                            if let Some(via) = device.exit_via.as_deref()
                                && via != "local"
                                && !labels.iter().any(|l| l == via)
                            {
                                labels.push(via.to_owned());
                            }
                            for label in labels {
                                let selected = device.exit_via.as_deref() == Some(label.as_str());
                                if ui
                                    .selectable_label(selected, format!("только через «{label}»"))
                                    .clicked()
                                {
                                    choice = Some(Some(label));
                                }
                            }
                            if let Some(choice) = choice {
                                match manager.set_device_exit(&device.name, choice.as_deref()) {
                                    Ok(()) => self.ok(format!("Выход «{}» сохранён.", device.name)),
                                    Err(error) => self.err(error),
                                }
                            }
                        });
                }
                if device.revoked {
                    ui.weak("отозвано");
                } else if ui.small_button("Отозвать").clicked() {
                    match manager.revoke(&device.name) {
                        Ok(()) => self.ok(format!(
                            "«{}» отозвано, его сессия будет закрыта.",
                            device.name
                        )),
                        Err(error) => self.err(error),
                    }
                }
            });
        }
    }
}
