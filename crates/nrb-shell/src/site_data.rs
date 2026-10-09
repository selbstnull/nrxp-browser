//! Окно «Данные сайтов» (N4a): какие сайты хранят куки, localStorage и sessionStorage, и удаление (по сайту или всё).
//! Данные берутся из `SiteDataManager` Servo (учитываются и обычное, и приватное хранилища). Сторонние куки — N4b в форке.

use nrb_engine_servo::servo::{SiteDataManager, StorageType};

#[derive(Clone, Debug)]
pub struct SiteRow {
    pub name: String,
    pub cookies: bool,
    pub local: bool,
    pub session: bool,
}

#[derive(Default)]
pub struct SiteDataWindow {
    pub open: bool,
    rows: Vec<SiteRow>,
    filter: String,
    loaded: bool,
}

fn all_types() -> StorageType {
    StorageType::Cookies | StorageType::Local | StorageType::Session
}

impl SiteDataWindow {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.loaded = false;
    }

    fn refresh(&mut self, manager: &SiteDataManager) {
        self.rows = manager
            .site_data(all_types())
            .into_iter()
            .map(|site| {
                let types = site.storage_types();
                SiteRow {
                    name: site.name(),
                    cookies: types.contains(StorageType::Cookies),
                    local: types.contains(StorageType::Local),
                    session: types.contains(StorageType::Session),
                }
            })
            .collect();
        self.loaded = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, manager: &SiteDataManager) {
        if !self.open {
            return;
        }
        if !self.loaded {
            self.refresh(manager);
        }
        let mut keep_open = true;
        let mut cleared = false;
        egui::Window::new("Данные сайтов")
            .open(&mut keep_open)
            .default_size([560.0, 420.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Фильтр");
                    ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(220.0));
                    if ui.button("Обновить").clicked() {
                        self.loaded = false;
                    }
                    if ui.button("Очистить всё").clicked() {
                        let names: Vec<&str> = self.rows.iter().map(|r| r.name.as_str()).collect();
                        manager.clear_site_data(&names, all_types());
                        cleared = true;
                    }
                });
                ui.weak("Куки, localStorage и sessionStorage (обычные и приватные вкладки). История и закладки здесь не затрагиваются.");
                ui.separator();
                let filter = self.filter.trim().to_lowercase();
                let rows: Vec<SiteRow> = self
                    .rows
                    .iter()
                    .filter(|r| filter.is_empty() || r.name.to_lowercase().contains(&filter))
                    .cloned()
                    .collect();
                if rows.is_empty() {
                    ui.weak("Нет сохранённых данных.");
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for row in rows {
                        ui.horizontal(|ui| {
                            ui.label(&row.name);
                            let mut badges = Vec::new();
                            if row.cookies {
                                badges.push("куки");
                            }
                            if row.local {
                                badges.push("localStorage");
                            }
                            if row.session {
                                badges.push("sessionStorage");
                            }
                            ui.weak(badges.join(" · "));
                            if ui.small_button("Удалить").clicked() {
                                manager.clear_site_data(&[row.name.as_str()], all_types());
                                cleared = true;
                            }
                        });
                    }
                });
            });
        if cleared {
            self.loaded = false;
        }
        if !keep_open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
        }
    }
}
