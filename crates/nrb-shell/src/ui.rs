//! Тулбар: полоса вкладок и навигационная строка. Только рисует и возвращает действия — состояния браузера не меняет.

pub struct TabInfo {
    pub icon: Option<egui::TextureId>,
    pub title: String,
    pub active: bool,
    pub loading: bool,
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
    /// Текущая страница в закладках (звезда закрашена).
    pub bookmarked: bool,
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
    ToggleBookmark,
    OpenHistory,
    OpenBookmarks,
    OpenSettings,
    OpenNodes,
    OpenDownloads,
    Find,
    OpenSiteData,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

/// Звезда закладки: рисуем сами (в шрифте egui нет «★/☆»).
fn star(ui: &mut egui::Ui, filled: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let center = rect.center();
    let points: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let angle = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
            let radius = if i % 2 == 0 { 9.0 } else { 4.0 };
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    let color = if filled {
        egui::Color32::from_rgb(250, 200, 50)
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    let fill = if filled {
        color
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        fill,
        egui::Stroke::new(1.5, color),
    ));
    response
}

/// Список подсказок под адресной строкой (история и закладки). Закрывается по клику мимо.
fn suggestions_popup(
    ctx: &egui::Context,
    url_bar: &egui::Response,
    state: &mut ToolbarState,
    actions: &mut Vec<Action>,
) {
    if !state.suggest_open || state.suggestions.is_empty() {
        return;
    }
    let area = egui::Area::new(egui::Id::new("url-suggestions"))
        .fixed_pos(url_bar.rect.left_bottom() + egui::vec2(0.0, 2.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(url_bar.rect.width().max(300.0));
                for (url, title) in &state.suggestions {
                    let text = if title.is_empty() {
                        url.clone()
                    } else {
                        format!("{}  —  {url}", short(title, 50))
                    };
                    if ui.selectable_label(false, short(&text, 110)).clicked() {
                        actions.push(Action::Navigate(url.clone()));
                        state.suggest_open = false;
                    }
                }
            });
        });
    let clicked_elsewhere = ctx.input(|i| i.pointer.any_click())
        && !ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| area.response.rect.contains(p) || url_bar.rect.contains(p));
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

pub fn toolbar(ui: &mut egui::Ui, tabs: &[TabInfo], state: &mut ToolbarState) -> Vec<Action> {
    let mut actions = Vec::new();
    egui::Panel::top("chrome").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (index, tab) in tabs.iter().enumerate() {
                if let Some(icon) = tab.icon {
                    ui.image((icon, egui::vec2(14.0, 14.0)));
                }
                let label = format!(
                    "{}{}",
                    if tab.loading { "⟳ " } else { "" },
                    short(&tab.title, 22)
                );
                if ui.selectable_label(tab.active, label).clicked() {
                    actions.push(Action::SelectTab(index));
                }
                if ui
                    .small_button("x")
                    .on_hover_text("Закрыть вкладку")
                    .clicked()
                {
                    actions.push(Action::CloseTab(index));
                }
                ui.add_space(6.0);
            }
            if ui
                .button("+")
                .on_hover_text("Новая вкладка (Ctrl+T)")
                .clicked()
            {
                actions.push(Action::NewTab);
            }
            if state.tunnel.is_some() || state.blocked.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((text, up)) = &state.tunnel {
                        let color = if *up {
                            egui::Color32::from_rgb(90, 200, 120)
                        } else {
                            egui::Color32::from_rgb(230, 90, 80)
                        };
                        ui.label(egui::RichText::new(text).color(color).small());
                        // Кружок рисуем сами: в шрифте egui нет символа «●».
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 4.0, color);
                    }
                    if let Some(count) = state.blocked {
                        ui.label(
                            egui::RichText::new(format!("заблокировано: {count}"))
                                .color(egui::Color32::from_rgb(150, 150, 170))
                                .small(),
                        );
                    }
                });
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(state.can_back, egui::Button::new("<"))
                .on_hover_text("Назад (Alt+←)")
                .clicked()
            {
                actions.push(Action::Back);
            }
            if ui
                .add_enabled(state.can_forward, egui::Button::new(">"))
                .on_hover_text("Вперёд (Alt+→)")
                .clicked()
            {
                actions.push(Action::Forward);
            }
            if ui
                .button("⟳")
                .on_hover_text("Перезагрузить (Ctrl+R)")
                .clicked()
            {
                actions.push(Action::Reload);
            }
            // Правая часть строки (меню, звезда) раскладывается справа налево, адресная строка занимает остаток.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("Меню", |ui| {
                    if ui.button("Журнал            Ctrl+H").clicked() {
                        actions.push(Action::OpenHistory);
                        ui.close();
                    }
                    if ui.button("Закладки          Ctrl+Shift+O").clicked() {
                        actions.push(Action::OpenBookmarks);
                        ui.close();
                    }
                    if ui.button("Настройки         Ctrl+,").clicked() {
                        actions.push(Action::OpenSettings);
                        ui.close();
                    }
                    if ui.button("Узлы              Ctrl+Shift+N").clicked() {
                        actions.push(Action::OpenNodes);
                        ui.close();
                    }
                    if ui.button("Найти на странице Ctrl+F").clicked() {
                        actions.push(Action::Find);
                        ui.close();
                    }
                    if ui.button("Данные сайтов Ctrl+Shift+Del").clicked() {
                        actions.push(Action::OpenSiteData);
                        ui.close();
                    }
                    if ui.button("Загрузки          Ctrl+J").clicked() {
                        actions.push(Action::OpenDownloads);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Масштаб +         Ctrl+=").clicked() {
                        actions.push(Action::ZoomIn);
                    }
                    if ui.button("Масштаб −         Ctrl+-").clicked() {
                        actions.push(Action::ZoomOut);
                    }
                    if ui.button("Масштаб 100%      Ctrl+0").clicked() {
                        actions.push(Action::ZoomReset);
                        ui.close();
                    }
                });
                if star(ui, state.bookmarked)
                    .on_hover_text("Добавить в закладки / убрать (Ctrl+D)")
                    .clicked()
                {
                    actions.push(Action::ToggleBookmark);
                }
                let edit = egui::TextEdit::singleline(&mut state.url_input)
                    .id(egui::Id::new("url-bar"))
                    .hint_text("Введите адрес или запрос")
                    .desired_width(ui.available_width());
                let response = ui.add(edit);
                if response.changed() {
                    state.suggest_open = true;
                    state.input_changed = true;
                }
                if state.focus_url {
                    state.focus_url = false;
                    response.request_focus();
                    if let Some(mut text_state) = egui::TextEdit::load_state(ui.ctx(), response.id)
                    {
                        let end = state.url_input.chars().count();
                        text_state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(end),
                            )));
                        text_state.store(ui.ctx(), response.id);
                    }
                }
                let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                if response.lost_focus() && enter {
                    state.suggest_open = false;
                    actions.push(Action::Navigate(state.url_input.clone()));
                }
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    state.suggest_open = false;
                }
                suggestions_popup(ui.ctx(), &response, state, &mut actions);
            });
        });
        ui.add_space(2.0);
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
