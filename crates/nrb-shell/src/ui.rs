//! Тулбар: полоса вкладок и навигационная строка. Только рисует и возвращает действия — состояния браузера не меняет.

pub struct TabInfo {
    pub icon: Option<egui::TextureId>,
    pub title: String,
    pub active: bool,
    pub loading: bool,
    /// Звук вкладки: что-то звучит / вкладка заглушена.
    pub audio: TabAudioState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabAudioState {
    #[default]
    Silent,
    Playing,
    Muted,
}

pub struct ToolbarState {
    pub url_input: String,
    /// Ctrl+L: перевести фокус в адресную строку и выделить текст.
    pub focus_url: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub loading: bool,
    /// Индикатор туннеля (текст, жив ли) — справа в строке вкладок.
    pub tunnel: Option<(String, bool)>,
    /// Сколько запросов заблокировано блокировщиком с запуска (`None` — блокировщик выключен).
    pub blocked: Option<u64>,
    /// Короткое сообщение о ходе длительной операции (печать в PDF) — справа в строке вкладок.
    pub notice: Option<String>,
    /// Текущая страница в закладках (звезда закрашена).
    pub bookmarked: bool,
    /// На странице есть форма с паролем и менеджер паролей доступен — показываем кнопку подстановки.
    pub login_form: bool,
    /// Подсказки адресной строки: `(адрес, заголовок)`.
    pub suggestions: Vec<(String, String)>,
    /// Показывается ли список подсказок (пользователь печатает в адресной строке).
    pub suggest_open: bool,
    /// Текст в адресной строке изменён пользователем (для пересчёта подсказок).
    pub input_changed: bool,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Navigate(String),
    Back,
    Forward,
    Reload,
    NewTab,
    CloseTab(usize),
    SelectTab(usize),
    /// Включить/выключить звук вкладки (значок динамика).
    ToggleMute(usize),
    /// Окно «Пароли» (Ctrl+Shift+P).
    OpenPasswords,
    /// Подставить сохранённый вход в форму страницы (Ctrl+Shift+L).
    FillPassword,
    ToggleBookmark,
    OpenHistory,
    OpenBookmarks,
    OpenSettings,
    OpenNodes,
    OpenDownloads,
    Find,
    OpenSiteData,
    Print,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, Ui, vec2,
};

use crate::theme::{self, Palette};

/// Значки интерфейса: рисуем сами (в шрифте egui нет стрелок, звезды, замка и т. п.), линиями одной толщины.
#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Back,
    Forward,
    Reload,
    Menu,
    Plus,
    Close,
    Key,
    Lock,
    Warning,
    Star(bool),
    Speaker(bool),
}

fn stroke(color: Color32) -> Stroke {
    Stroke::new(1.6, color)
}

fn arc(center: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let steps = 24;
    (0..=steps)
        .map(|i| {
            let a = from + (to - from) * (i as f32 / steps as f32);
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect()
}

fn draw_icon(painter: &egui::Painter, icon: Icon, rect: Rect, color: Color32, p: &Palette) {
    let c = rect.center();
    let st = stroke(color);
    let line = |a: Pos2, b: Pos2| painter.line_segment([a, b], st);
    match icon {
        Icon::Back | Icon::Forward => {
            let s = if icon == Icon::Back { 1.0 } else { -1.0 };
            line(c + vec2(7.0 * s, 0.0), c + vec2(-7.0 * s, 0.0));
            line(c + vec2(-7.0 * s, 0.0), c + vec2(-1.5 * s, -5.5));
            line(c + vec2(-7.0 * s, 0.0), c + vec2(-1.5 * s, 5.5));
        }
        Icon::Reload => {
            // Дуга по часовой стрелке с просветом сверху справа и наконечником на конце.
            let r = 6.6;
            let (from, to) = (-0.55_f32, 4.15_f32);
            painter.add(Shape::line(arc(c, r, from, to), st));
            let end = c + vec2(to.cos(), to.sin()) * r;
            let tangent = vec2(-to.sin(), to.cos());
            let tip = end + tangent * 2.5;
            for angle in [0.75_f32, -0.75] {
                let (sin, cos) = angle.sin_cos();
                let back = vec2(
                    -tangent.x * cos + tangent.y * sin,
                    -tangent.x * sin - tangent.y * cos,
                );
                line(tip, tip + back * 5.2);
            }
        }
        Icon::Menu => {
            for y in [-5.5, 0.0, 5.5] {
                line(c + vec2(-7.5, y), c + vec2(7.5, y));
            }
        }
        Icon::Plus => {
            line(c + vec2(-6.0, 0.0), c + vec2(6.0, 0.0));
            line(c + vec2(0.0, -6.0), c + vec2(0.0, 6.0));
        }
        Icon::Close => {
            line(c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5));
            line(c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5));
        }
        Icon::Key => {
            painter.circle_stroke(c + vec2(-3.5, -3.5), 4.2, st);
            line(c + vec2(-0.5, -0.5), c + vec2(7.0, 7.0));
            line(c + vec2(4.5, 4.5), c + vec2(7.0, 2.0));
        }
        Icon::Lock => {
            let body = Rect::from_center_size(c + vec2(0.0, 2.5), vec2(11.0, 8.0));
            painter.rect_stroke(body, 2.0, Stroke::new(1.4, color), StrokeKind::Inside);
            painter.add(Shape::line(
                arc(
                    c + vec2(0.0, -1.5),
                    3.6,
                    std::f32::consts::PI,
                    std::f32::consts::TAU,
                ),
                Stroke::new(1.4, color),
            ));
        }
        Icon::Warning => {
            let pts = vec![
                c + vec2(0.0, -6.5),
                c + vec2(6.8, 5.5),
                c + vec2(-6.8, 5.5),
                c + vec2(0.0, -6.5),
            ];
            painter.add(Shape::line(pts, Stroke::new(1.4, color)));
            line(c + vec2(0.0, -2.0), c + vec2(0.0, 1.5));
            painter.circle_filled(c + vec2(0.0, 3.8), 0.9, color);
        }
        Icon::Star(filled) => {
            let points: Vec<Pos2> = (0..10)
                .map(|i| {
                    let a = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
                    let r = if i % 2 == 0 { 8.0 } else { 3.6 };
                    c + vec2(a.cos(), a.sin()) * r
                })
                .collect();
            let col = if filled { p.warn } else { color };
            painter.add(Shape::convex_polygon(
                points,
                if filled { col } else { Color32::TRANSPARENT },
                Stroke::new(1.4, col),
            ));
        }
        Icon::Speaker(muted) => {
            let col = if muted { p.danger } else { color };
            painter.add(Shape::convex_polygon(
                vec![
                    c + vec2(-6.0, -2.5),
                    c + vec2(-3.0, -2.5),
                    c + vec2(1.0, -6.0),
                    c + vec2(1.0, 6.0),
                    c + vec2(-3.0, 2.5),
                    c + vec2(-6.0, 2.5),
                ],
                col,
                Stroke::NONE,
            ));
            let s = Stroke::new(1.4, col);
            if muted {
                painter.line_segment([c + vec2(3.5, -3.5), c + vec2(8.0, 3.5)], s);
                painter.line_segment([c + vec2(3.5, 3.5), c + vec2(8.0, -3.5)], s);
            } else {
                for radius in [3.5_f32, 6.5] {
                    painter.add(Shape::line(
                        (-3..=3)
                            .map(|i| {
                                let a = i as f32 * 0.25;
                                c + vec2(2.0 + a.cos() * radius, a.sin() * radius)
                            })
                            .collect(),
                        s,
                    ));
                }
            }
        }
    }
}

/// Квадратная кнопка-значок без рамки: при наведении появляется подложка.
fn icon_button(ui: &mut Ui, p: &Palette, icon: Icon, enabled: bool, size: f32) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), sense);
    if ui.is_rect_visible(rect) {
        let active = enabled && (response.hovered() || response.is_pointer_button_down_on());
        if active {
            ui.painter().rect_filled(rect, 6.0, p.hover);
        }
        let color = if !enabled {
            p.muted.gamma_multiply(0.45)
        } else if active {
            p.text
        } else {
            p.text.gamma_multiply(0.82)
        };
        draw_icon(ui.painter(), icon, rect, color, p);
    }
    response
}

/// Список подсказок под адресной строкой (история и закладки). Закрывается по клику мимо.
fn suggestions_popup(
    ctx: &egui::Context,
    url_bar: Rect,
    state: &mut ToolbarState,
    actions: &mut Vec<Action>,
) {
    if !state.suggest_open || state.suggestions.is_empty() {
        return;
    }
    let p = theme::current(ctx);
    let area = egui::Area::new(egui::Id::new("url-suggestions"))
        .fixed_pos(url_bar.left_bottom() + vec2(0.0, 4.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(url_bar.width().max(300.0) - 12.0);
                    for (url, title) in &state.suggestions {
                        let (rect, response) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 6.0, p.hover);
                        }
                        let label = if title.is_empty() {
                            short(url, 70)
                        } else {
                            short(title, 46)
                        };
                        let galley =
                            ui.painter()
                                .layout_no_wrap(label, FontId::proportional(14.0), p.text);
                        let text_w = galley.size().x;
                        ui.painter().galley(
                            rect.left_center() + vec2(10.0, -galley.size().y / 2.0),
                            galley,
                            p.text,
                        );
                        if !title.is_empty() {
                            let max = (rect.width() - text_w - 40.0).max(0.0);
                            if max > 40.0 {
                                let url_text = ui.painter().layout_no_wrap(
                                    short(url, (max / 6.5) as usize),
                                    FontId::proportional(12.5),
                                    p.muted,
                                );
                                ui.painter().galley(
                                    rect.left_center()
                                        + vec2(20.0 + text_w, -url_text.size().y / 2.0),
                                    url_text,
                                    p.muted,
                                );
                            }
                        }
                        if response.clicked() {
                            actions.push(Action::Navigate(url.clone()));
                            state.suggest_open = false;
                        }
                    }
                });
        });
    let clicked_elsewhere = ctx.input(|i| i.pointer.any_click())
        && !ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| area.response.rect.contains(pos) || url_bar.contains(pos));
    if clicked_elsewhere {
        state.suggest_open = false;
    }
}

fn short(title: &str, max: usize) -> String {
    if title.chars().count() <= max {
        title.to_owned()
    } else {
        let cut: String = title.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Строка пункта меню: название слева, сочетание клавиш справа.
fn menu_item(ui: &mut Ui, p: &Palette, label: &str, shortcut: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, p.hover);
    }
    ui.painter().text(
        rect.left_center() + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        p.text,
    );
    if !shortcut.is_empty() {
        ui.painter().text(
            rect.right_center() - vec2(10.0, 0.0),
            Align2::RIGHT_CENTER,
            shortcut,
            FontId::proportional(12.0),
            p.muted,
        );
    }
    response
}

fn menu_separator(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().line_segment(
        [
            rect.left_center() + vec2(6.0, 0.0),
            rect.right_center() - vec2(6.0, 0.0),
        ],
        Stroke::new(1.0, p.border),
    );
}

/// Плашка с текстом справа в строке вкладок (индикатор туннеля).
fn pill(ui: &mut Ui, p: &Palette, text: &str, dot: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(12.0), p.muted);
    let size = vec2(galley.size().x + 30.0, 24.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_stroke(rect, 12.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    ui.painter()
        .circle_filled(rect.left_center() + vec2(12.0, 0.0), 3.5, dot);
    ui.painter().galley(
        rect.left_center() + vec2(22.0, -galley.size().y / 2.0),
        galley,
        p.muted,
    );
}

/// Вкладка: фавикон (или индикатор загрузки), заголовок с многоточием, значок звука, крестик (у активной и при наведении).
fn tab_widget(
    ui: &mut Ui,
    p: &Palette,
    index: usize,
    info: &TabInfo,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let height = 34.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let id = ui.id().with(("tab", index));
    let response = ui.interact(rect, id, Sense::click());
    let hovered = response.hovered();

    // Подложка: активная вкладка с закруглёнными верхними углами и тонкой акцентной линией, наведённая — «плашка».
    if info.active {
        ui.painter().rect_filled(
            rect,
            CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            },
            p.surface,
        );
        ui.painter().rect_filled(
            Rect::from_min_size(
                rect.left_top() + vec2(10.0, 0.0),
                vec2(rect.width() - 20.0, 2.0),
            ),
            1.0,
            p.accent,
        );
    } else if hovered {
        ui.painter().rect_filled(
            rect.shrink2(vec2(1.0, 3.0)),
            6.0,
            p.hover.gamma_multiply(0.7),
        );
    }

    let cy = rect.center().y;
    let close_rect = Rect::from_center_size(Pos2::new(rect.right() - 17.0, cy), vec2(22.0, 22.0));
    let show_close = info.active || hovered;
    let mut text_right = rect.right() - 10.0;
    if show_close {
        text_right = close_rect.left() - 4.0;
    }
    let audio_rect = Rect::from_center_size(Pos2::new(text_right - 11.0, cy), vec2(22.0, 22.0));
    if info.audio != TabAudioState::Silent {
        text_right = audio_rect.left() - 2.0;
    }

    // Фавикон / индикатор загрузки.
    let icon_c = Pos2::new(rect.left() + 18.0, cy);
    if info.loading {
        let t = ui.input(|i| i.time) as f32;
        let from = t * 6.0;
        ui.painter().add(Shape::line(
            arc(icon_c, 6.5, from, from + std::f32::consts::TAU * 0.72),
            Stroke::new(2.0, p.accent),
        ));
        ui.ctx().request_repaint();
    } else if let Some(texture) = info.icon {
        let r = Rect::from_center_size(icon_c, vec2(16.0, 16.0));
        ui.painter().image(
            texture,
            r,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        ui.painter()
            .circle_stroke(icon_c, 6.0, Stroke::new(1.3, p.muted));
    }

    // Заголовок.
    let color = if info.active || hovered {
        p.text
    } else {
        p.muted
    };
    let title_left = rect.left() + 34.0;
    let max_w = (text_right - title_left).max(8.0);
    let mut job = egui::text::LayoutJob::single_section(
        if info.title.is_empty() {
            "Новая вкладка".to_owned()
        } else {
            info.title.clone()
        },
        egui::TextFormat {
            font_id: FontId::proportional(13.5),
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        Pos2::new(title_left, cy - galley.size().y / 2.0),
        galley,
        color,
    );

    // Звук.
    if info.audio != TabAudioState::Silent {
        let muted = info.audio == TabAudioState::Muted;
        let audio = ui.interact(audio_rect, id.with("audio"), Sense::click());
        if audio.hovered() {
            ui.painter().rect_filled(audio_rect, 6.0, p.hover);
        }
        draw_icon(
            ui.painter(),
            Icon::Speaker(muted),
            audio_rect,
            if audio.hovered() { p.text } else { p.muted },
            p,
        );
        if audio
            .on_hover_text(if muted {
                "Включить звук вкладки"
            } else {
                "Отключить звук вкладки"
            })
            .clicked()
        {
            actions.push(Action::ToggleMute(index));
            return;
        }
    }

    // Крестик (поверх остального, чтобы клик по нему не выбирал вкладку).
    if show_close {
        let close = ui.interact(close_rect, id.with("close"), Sense::click());
        if close.hovered() {
            ui.painter().rect_filled(close_rect, 6.0, p.hover);
        }
        draw_icon(
            ui.painter(),
            Icon::Close,
            close_rect,
            if close.hovered() { p.text } else { p.muted },
            p,
        );
        if close.on_hover_text("Закрыть вкладку (Ctrl+W)").clicked() {
            actions.push(Action::CloseTab(index));
            return;
        }
    }

    if response.middle_clicked() {
        actions.push(Action::CloseTab(index));
    } else if response.clicked() {
        actions.push(Action::SelectTab(index));
    }
}

pub fn toolbar(ui: &mut Ui, tabs: &[TabInfo], state: &mut ToolbarState) -> Vec<Action> {
    let mut actions = Vec::new();
    let p = theme::current(ui.ctx());
    egui::Panel::top("chrome")
        .frame(egui::Frame::new().fill(p.strip))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

            // ── Полоса вкладок ──────────────────────────────────────────────────────────────────────────────────────────────
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let status_w = if state.tunnel.is_some()
                    || state.blocked.is_some()
                    || state.notice.is_some()
                {
                    300.0
                } else {
                    16.0
                };
                let free = (ui.available_width() - status_w - 44.0).max(120.0);
                let tab_w = (free / tabs.len().max(1) as f32).clamp(96.0, 230.0);
                for (index, tab) in tabs.iter().enumerate() {
                    tab_widget(ui, &p, index, tab, tab_w, &mut actions);
                }
                ui.add_space(4.0);
                let plus = ui.scope(|ui| {
                    ui.add_space(0.0);
                    icon_button(ui, &p, Icon::Plus, true, 30.0)
                });
                if plus.inner.on_hover_text("Новая вкладка (Ctrl+T)").clicked() {
                    actions.push(Action::NewTab);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(10.0);
                    ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                    if let Some((text, up)) = &state.tunnel {
                        pill(ui, &p, text, if *up { p.ok } else { p.danger });
                    }
                    if let Some(notice) = &state.notice {
                        ui.label(egui::RichText::new(notice).color(p.warn).size(12.0));
                    }
                    if let Some(count) = state.blocked {
                        ui.label(
                            egui::RichText::new(format!("заблокировано: {count}"))
                                .color(p.muted)
                                .size(12.0),
                        );
                    }
                });
            });

            // ── Панель инструментов ─────────────────────────────────────────────────────────────────────────────────────────
            let toolbar_rect = egui::Frame::new()
                .fill(p.surface)
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                        if icon_button(ui, &p, Icon::Back, state.can_back, 32.0)
                            .on_hover_text("Назад (Alt+←)")
                            .clicked()
                        {
                            actions.push(Action::Back);
                        }
                        if icon_button(ui, &p, Icon::Forward, state.can_forward, 32.0)
                            .on_hover_text("Вперёд (Alt+→)")
                            .clicked()
                        {
                            actions.push(Action::Forward);
                        }
                        if icon_button(ui, &p, Icon::Reload, true, 32.0)
                            .on_hover_text("Перезагрузить (Ctrl+R)")
                            .clicked()
                        {
                            actions.push(Action::Reload);
                        }
                        ui.add_space(6.0);

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let menu =
                                icon_button(ui, &p, Icon::Menu, true, 32.0).on_hover_text("Меню");
                            egui::Popup::menu(&menu)
                                .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
                                .width(300.0)
                                .show(|ui| {
                                    ui.set_min_width(290.0);
                                    let mut add = |ui: &mut Ui,
                                                   label: &str,
                                                   shortcut: &str,
                                                   action: Action,
                                                   close: bool,
                                                   actions: &mut Vec<Action>| {
                                        if menu_item(ui, &p, label, shortcut).clicked() {
                                            actions.push(action);
                                            if close {
                                                ui.close();
                                            }
                                        }
                                    };
                                    add(
                                        ui,
                                        "Закладки",
                                        "Ctrl+Shift+O",
                                        Action::OpenBookmarks,
                                        true,
                                        &mut actions,
                                    );
                                    add(
                                        ui,
                                        "Журнал",
                                        "Ctrl+H",
                                        Action::OpenHistory,
                                        true,
                                        &mut actions,
                                    );
                                    add(
                                        ui,
                                        "Загрузки",
                                        "Ctrl+J",
                                        Action::OpenDownloads,
                                        true,
                                        &mut actions,
                                    );
                                    add(
                                        ui,
                                        "Пароли",
                                        "Ctrl+Shift+P",
                                        Action::OpenPasswords,
                                        true,
                                        &mut actions,
                                    );
                                    menu_separator(ui, &p);
                                    add(
                                        ui,
                                        "Найти на странице",
                                        "Ctrl+F",
                                        Action::Find,
                                        true,
                                        &mut actions,
                                    );
                                    add(
                                        ui,
                                        "Печать в PDF",
                                        "Ctrl+P",
                                        Action::Print,
                                        true,
                                        &mut actions,
                                    );
                                    add(
                                        ui,
                                        "Данные сайтов",
                                        "Ctrl+Shift+Del",
                                        Action::OpenSiteData,
                                        true,
                                        &mut actions,
                                    );
                                    menu_separator(ui, &p);
                                    add(
                                        ui,
                                        "Увеличить",
                                        "Ctrl+=",
                                        Action::ZoomIn,
                                        false,
                                        &mut actions,
                                    );
                                    add(
                                        ui,
                                        "Уменьшить",
                                        "Ctrl+−",
                                        Action::ZoomOut,
                                        false,
                                        &mut actions,
                                    );
                                    add(
                                        ui,
                                        "Масштаб 100%",
                                        "Ctrl+0",
                                        Action::ZoomReset,
                                        true,
                                        &mut actions,
                                    );
                                    menu_separator(ui, &p);
                                    add(
                                        ui,
                                        "Узлы",
                                        "Ctrl+Shift+N",
                                        Action::OpenNodes,
                                        true,
                                        &mut actions,
                                    );
                                    add(
                                        ui,
                                        "Настройки",
                                        "Ctrl+,",
                                        Action::OpenSettings,
                                        true,
                                        &mut actions,
                                    );
                                });
                            if state.login_form
                                && icon_button(ui, &p, Icon::Key, true, 32.0)
                                    .on_hover_text("Подставить сохранённый вход (Ctrl+Shift+L)")
                                    .clicked()
                            {
                                actions.push(Action::FillPassword);
                            }
                            ui.add_space(6.0);

                            // Адресная строка: скруглённое поле; слева значок защищённости, справа — звезда.
                            let width = ui.available_width();
                            let (bar, _) =
                                ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
                            let focused = ui.memory(|m| m.has_focus(egui::Id::new("url-bar")));
                            ui.painter().rect_filled(bar, 8.0, p.field);
                            ui.painter().rect_stroke(
                                bar,
                                8.0,
                                if focused {
                                    Stroke::new(1.6, p.accent)
                                } else {
                                    Stroke::new(1.0, p.border)
                                },
                                StrokeKind::Inside,
                            );
                            let inner = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(bar.shrink2(vec2(4.0, 0.0)))
                                    .layout(Layout::left_to_right(Align::Center)),
                            );
                            let mut inner = inner;
                            let inner = &mut inner;
                            let secure = state.url_input.starts_with("https://");
                            let insecure = state.url_input.starts_with("http://");
                            if secure || insecure {
                                let (r, resp) =
                                    inner.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                                draw_icon(
                                    inner.painter(),
                                    if secure { Icon::Lock } else { Icon::Warning },
                                    r,
                                    if secure { p.muted } else { p.warn },
                                    &p,
                                );
                                resp.on_hover_text(if secure {
                                    "Защищённое соединение (HTTPS)"
                                } else {
                                    "Соединение не защищено (HTTP)"
                                });
                            } else {
                                inner.add_space(8.0);
                            }
                            inner.spacing_mut().item_spacing = vec2(0.0, 0.0);
                            let star_w = 30.0;
                            let edit = egui::TextEdit::singleline(&mut state.url_input)
                                .id(egui::Id::new("url-bar"))
                                .hint_text("Введите адрес или запрос")
                                .frame(egui::Frame::NONE)
                                .margin(Margin::symmetric(4, 0))
                                .vertical_align(Align::Center)
                                .min_size(vec2(0.0, 30.0))
                                .desired_width((inner.available_width() - star_w).max(40.0));
                            let response = inner.add(edit);
                            if icon_button(inner, &p, Icon::Star(state.bookmarked), true, 26.0)
                                .on_hover_text("Добавить в закладки / убрать (Ctrl+D)")
                                .clicked()
                            {
                                actions.push(Action::ToggleBookmark);
                            }
                            if response.changed() {
                                state.suggest_open = true;
                                state.input_changed = true;
                            }
                            if state.focus_url {
                                state.focus_url = false;
                                response.request_focus();
                                if let Some(mut text_state) =
                                    egui::TextEdit::load_state(inner.ctx(), response.id)
                                {
                                    let end = state.url_input.chars().count();
                                    text_state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(0),
                                            egui::text::CCursor::new(end),
                                        ),
                                    ));
                                    text_state.store(inner.ctx(), response.id);
                                }
                            }
                            let enter = inner.input(|input| input.key_pressed(egui::Key::Enter));
                            if response.lost_focus() && enter {
                                state.suggest_open = false;
                                actions.push(Action::Navigate(state.url_input.clone()));
                            }
                            if inner.input(|input| input.key_pressed(egui::Key::Escape)) {
                                state.suggest_open = false;
                            }
                            suggestions_popup(inner.ctx(), bar, state, &mut actions);
                        });
                    });
                })
                .response
                .rect;

            // Нижняя граница панели и индикатор загрузки (бегущий отрезок акцентного цвета).
            let painter = ui.painter();
            painter.line_segment(
                [toolbar_rect.left_bottom(), toolbar_rect.right_bottom()],
                Stroke::new(1.0, p.border),
            );
            if state.loading {
                let t = ui.input(|i| i.time) as f32;
                let w = toolbar_rect.width();
                let seg = w * 0.28;
                let x = toolbar_rect.left() + ((t * 0.9) % 1.3 - 0.3) * w;
                let r = Rect::from_min_max(
                    Pos2::new(x.max(toolbar_rect.left()), toolbar_rect.bottom() - 2.0),
                    Pos2::new((x + seg).min(toolbar_rect.right()), toolbar_rect.bottom()),
                );
                if r.width() > 0.0 {
                    ui.painter().rect_filled(r, 1.0, p.accent);
                }
                ui.ctx().request_repaint();
            }
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::short;

    #[test]
    fn short_truncates_on_char_boundary() {
        assert_eq!(short("abc", 5), "abc");
        assert_eq!(short("Заголовок очень длинный", 10), "Заголовок…");
    }
}
