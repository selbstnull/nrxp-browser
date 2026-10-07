//! Окна «Журнал», «Закладки» и «Настройки» (egui поверх страницы). Данные берутся из `nrb-profile`.

use nrb_profile::{SEARCH_ENGINES, Settings, Store, Theme};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    History,
    Bookmarks,
    Settings,
}

pub enum LibraryAction {
    Open(String),
    SettingsChanged,
}

#[derive(Default)]
pub struct Library {
    pub open: Option<Panel>,
    query: String,
}

fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!(
            "{}…",
            text.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

impl Library {
    pub fn toggle(&mut self, panel: Panel) {
        self.open = if self.open == Some(panel) {
            None
        } else {
            Some(panel)
        };
        self.query.clear();
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        store: &Store,
        settings: &mut Settings,
    ) -> Vec<LibraryAction> {
        let mut actions = Vec::new();
        let Some(panel) = self.open else {
            return actions;
        };
        let mut keep_open = true;
        match panel {
            Panel::History => self.history(ctx, store, &mut keep_open, &mut actions),
            Panel::Bookmarks => Self::bookmarks(ctx, store, &mut keep_open, &mut actions),
            Panel::Settings => Self::settings(ctx, settings, &mut keep_open, &mut actions),
        }
        if !keep_open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = None;
        }
        actions
    }

    fn history(
        &mut self,
        ctx: &egui::Context,
        store: &Store,
        keep_open: &mut bool,
        actions: &mut Vec<LibraryAction>,
    ) {
        egui::Window::new("Журнал")
            .open(keep_open)
            .default_size([620.0, 460.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Поиск");
                    ui.text_edit_singleline(&mut self.query);
                    if ui.button("Очистить журнал").clicked() {
                        let _ = store.clear_history();
                    }
                });
                ui.separator();
                let entries = if self.query.trim().is_empty() {
                    store.recent_history(300)
                } else {
                    store.search_history(&self.query, 300)
                };
                egui::ScrollArea::vertical().show(ui, |ui| match entries {
                    Ok(entries) if entries.is_empty() => {
                        ui.label("Пока пусто.");
                    }
                    Ok(entries) => {
                        for entry in entries {
                            ui.horizontal(|ui| {
                                let label = if entry.title.is_empty() {
                                    entry.url.clone()
                                } else {
                                    entry.title.clone()
                                };
                                if ui
                                    .link(short(&label, 70))
                                    .on_hover_text(&entry.url)
                                    .clicked()
                                {
                                    actions.push(LibraryAction::Open(entry.url.clone()));
                                }
                                ui.weak(short(&entry.url, 40));
                                if ui
                                    .small_button("x")
                                    .on_hover_text("Удалить из журнала")
                                    .clicked()
                                {
                                    let _ = store.delete_history_entry(&entry.url);
                                }
                            });
                        }
                    }
                    Err(error) => {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!("Не удалось прочитать журнал: {error}"),
                        );
                    }
                });
            });
    }

    fn bookmarks(
        ctx: &egui::Context,
        store: &Store,
        keep_open: &mut bool,
        actions: &mut Vec<LibraryAction>,
    ) {
        egui::Window::new("Закладки")
            .open(keep_open)
            .default_size([620.0, 420.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match store.bookmarks() {
                    Ok(list) if list.is_empty() => {
                        ui.label("Закладок нет. Ctrl+D — добавить текущую страницу.");
                    }
                    Ok(list) => {
                        for bookmark in list {
                            ui.horizontal(|ui| {
                                let label = if bookmark.title.is_empty() {
                                    bookmark.url.clone()
                                } else {
                                    bookmark.title.clone()
                                };
                                if ui
                                    .link(short(&label, 70))
                                    .on_hover_text(&bookmark.url)
                                    .clicked()
                                {
                                    actions.push(LibraryAction::Open(bookmark.url.clone()));
                                }
                                ui.weak(short(&bookmark.url, 40));
                                if ui
                                    .small_button("x")
                                    .on_hover_text("Удалить закладку")
                                    .clicked()
                                {
                                    let _ = store.remove_bookmark(&bookmark.url);
                                }
                            });
                        }
                    }
                    Err(error) => {
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!("Не удалось прочитать закладки: {error}"),
                        );
                    }
                });
            });
    }

    fn settings(
        ctx: &egui::Context,
        settings: &mut Settings,
        keep_open: &mut bool,
        actions: &mut Vec<LibraryAction>,
    ) {
        egui::Window::new("Настройки").open(keep_open).default_size([520.0, 360.0]).show(ctx, |ui| {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Поисковая система");
                let current = SEARCH_ENGINES.iter().find(|(_, template)| *template == settings.search).map_or("Своя", |(name, _)| *name);
                egui::ComboBox::from_id_salt("search-engine").selected_text(current).show_ui(ui, |ui| {
                    for (name, template) in SEARCH_ENGINES {
                        if ui.selectable_label(settings.search == *template, *name).clicked() {
                            settings.search = (*template).to_owned();
                            changed = true;
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Шаблон поиска");
                changed |= ui.add(egui::TextEdit::singleline(&mut settings.search).desired_width(f32::INFINITY)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("Домашняя страница");
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut settings.homepage).hint_text("пусто — пустая вкладка").desired_width(f32::INFINITY))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Тема");
                changed |= ui.radio_value(&mut settings.theme, Theme::Dark, "Тёмная").changed();
                changed |= ui.radio_value(&mut settings.theme, Theme::Light, "Светлая").changed();
            });
            ui.horizontal(|ui| {
                ui.label("Масштаб по умолчанию");
                changed |= ui.add(egui::Slider::new(&mut settings.default_zoom, 0.5..=3.0).fixed_decimals(2)).changed();
            });
            changed |= ui.checkbox(&mut settings.restore_session, "Восстанавливать вкладки при запуске").changed();
            changed |= ui.checkbox(&mut settings.record_history, "Вести журнал посещений").changed();
            changed |= ui.checkbox(&mut settings.adblock, "Блокировать рекламу и трекеры (списки *.txt в каталоге filters/ профиля)").changed();
            ui.separator();
            ui.weak("Настройки сохраняются в settings.toml в каталоге профиля. Поисковая система и тема применяются сразу.");
            if changed {
                actions.push(LibraryAction::SettingsChanged);
            }
        });
    }
}
