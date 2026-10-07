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
            if let Some((text, up)) = &state.tunnel {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
            let edit = egui::TextEdit::singleline(&mut state.url_input)
                .id(egui::Id::new("url-bar"))
                .hint_text("Введите адрес или запрос")
                .desired_width(f32::INFINITY);
            let response = ui.add(edit);
            if state.focus_url {
                state.focus_url = false;
                response.request_focus();
                if let Some(mut text_state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
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
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                actions.push(Action::Navigate(state.url_input.clone()));
            }
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
