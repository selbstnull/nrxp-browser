//! Поиск по странице: Ctrl+F. Поиск, подсветка и переходы — в движке форка (`WebView::find`, N5b): DOM страницы, выделение
//! пользователя и MutationObserver не затрагиваются. Здесь — только панель и разбор ответа. Ограничения движка: главный
//! документ (без iframe), без shadow DOM, подсветка пропадает при перестройке текста (см. `PLAN.md` форка).

use std::cell::RefCell;
use std::rc::Rc;

/// Что вернул скрипт: сколько совпадений и какое активно (с нуля; `-1` — нет).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindResult {
    pub count: usize,
    pub active: Option<usize>,
}

/// Общая ячейка для результата: колбэк `evaluate_javascript` вызывается позже, из цикла Servo.
pub type SharedResult = Rc<RefCell<Option<FindResult>>>;

/// Разбор ответа движка `{"count":N,"active":I|null}`.
pub fn parse_result(json: &str) -> Option<FindResult> {
    let raw = |key: &str| -> Option<&str> {
        let at = json.find(&format!("\"{key}\":"))? + key.len() + 3;
        let rest = &json[at..];
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    };
    let count = raw("count")?.parse::<i64>().ok()?.max(0) as usize;
    // `null` — совпадений нет.
    let active = raw("active")?.parse::<i64>().ok().filter(|a| *a >= 0);
    Some(FindResult {
        count,
        active: active.map(|a| a as usize),
    })
}

pub enum FindAction {
    Search(String, bool),
    Step(i32),
    Close,
}

#[derive(Default)]
pub struct FindBar {
    pub open: bool,
    query: String,
    case_sensitive: bool,
    /// Запрос, по которому последний раз искали (чтобы не искать заново на каждый кадр).
    searched: Option<(String, bool)>,
    pub result: Option<FindResult>,
    pub shared: SharedResult,
    focus: bool,
}

impl FindBar {
    pub fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }

    /// Забыть результат (страница сменилась — метки пропали вместе с DOM).
    /// Текущий запрос и учёт регистра — для шагов «дальше/назад» (движку каждый раз нужен запрос).
    pub fn query(&self) -> (&str, bool) {
        (&self.query, self.case_sensitive)
    }

    pub fn reset_for_new_page(&mut self) {
        self.searched = None;
        self.result = None;
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Vec<FindAction> {
        let mut actions = Vec::new();
        if let Some(result) = self.shared.borrow_mut().take() {
            self.result = Some(result);
        }
        if !self.open {
            return actions;
        }
        ui.horizontal(|ui| {
            ui.label("Найти:");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .desired_width(260.0)
                    .hint_text("текст на странице"),
            );
            if self.focus {
                response.request_focus();
                self.focus = false;
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let shift = ui.input(|i| i.modifiers.shift);
            let case_changed = ui
                .checkbox(&mut self.case_sensitive, "Aa")
                .on_hover_text("Учитывать регистр")
                .changed();
            let current = (self.query.clone(), self.case_sensitive);
            if self.searched.as_ref() != Some(&current) || case_changed {
                self.searched = Some(current.clone());
                actions.push(FindAction::Search(current.0, current.1));
            } else if enter {
                actions.push(FindAction::Step(if shift { -1 } else { 1 }));
                response.request_focus();
            }
            if ui
                .button("↑")
                .on_hover_text("Предыдущее (Shift+Enter)")
                .clicked()
            {
                actions.push(FindAction::Step(-1));
            }
            if ui
                .button("След.")
                .on_hover_text("Следующее (Enter)")
                .clicked()
            {
                actions.push(FindAction::Step(1));
            }
            match (&self.result, self.query.is_empty()) {
                (_, true) => {}
                (Some(r), _) if r.count == 0 => {
                    ui.colored_label(egui::Color32::LIGHT_RED, "ничего не найдено");
                }
                (Some(r), _) => {
                    ui.label(format!("{} из {}", r.active.map_or(0, |a| a + 1), r.count));
                }
                (None, _) => {
                    ui.weak("…");
                }
            }
            if ui
                .button("Закрыть")
                .on_hover_text("Закрыть (Esc)")
                .clicked()
                || ui.input(|i| i.key_pressed(egui::Key::Escape))
            {
                actions.push(FindAction::Close);
                self.open = false;
            }
        });
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_parsed() {
        assert_eq!(
            parse_result("{\"count\":12,\"active\":3}"),
            Some(FindResult {
                count: 12,
                active: Some(3)
            })
        );
        assert_eq!(
            parse_result("{\"count\":0,\"active\":-1}"),
            Some(FindResult {
                count: 0,
                active: None
            })
        );
        assert_eq!(
            parse_result("{\"count\":0,\"active\":null}"),
            Some(FindResult {
                count: 0,
                active: None
            })
        );
        assert_eq!(parse_result("мусор"), None);
    }
}
