//! Модальные запросы Servo к оболочке: диалоги страницы (alert/confirm/prompt), разрешения, HTTP-авторизация,
//! выпадающие списки `<select>` и контекстное меню. Всё рисуется egui поверх страницы.
//!
//! Сообщения диалогов задаёт страница, поэтому окно всегда подписано адресом страницы-источника — его нельзя принять
//! за интерфейс браузера.

use nrb_engine_servo::servo::{
    AuthenticationRequest, ContextMenu, ContextMenuAction, ContextMenuItem, PermissionFeature,
    PermissionRequest, SelectElement, SelectElementOptionOrOptgroup, SimpleDialog,
};

/// Группа опций `<select>`: заголовок (если это optgroup) и опции `(id, подпись, недоступна)`.
type OptionGroup = (Option<String>, Vec<(usize, String, bool)>);

pub enum Modal {
    Dialog {
        origin: String,
        dialog: Option<SimpleDialog>,
        input: String,
    },
    Permission {
        origin: String,
        feature: PermissionFeature,
        request: Option<PermissionRequest>,
    },
    Auth {
        url: String,
        for_proxy: bool,
        request: Option<AuthenticationRequest>,
        user: String,
        pass: String,
    },
    Select {
        element: Option<SelectElement>,
        selected: Vec<usize>,
        /// Левый нижний угол элемента в точках окна (с учётом тулбара).
        anchor: egui::Pos2,
    },
    Context {
        menu: Option<ContextMenu>,
        anchor: egui::Pos2,
    },
}

pub fn feature_name(feature: PermissionFeature) -> String {
    match feature {
        PermissionFeature::Geolocation => "определить ваше местоположение",
        PermissionFeature::Notifications => "показывать уведомления",
        PermissionFeature::Push => "присылать push-сообщения",
        PermissionFeature::Midi => "использовать MIDI-устройства",
        PermissionFeature::Camera => "использовать камеру",
        PermissionFeature::Microphone => "использовать микрофон",
        PermissionFeature::Speaker => "выбирать устройство вывода звука",
        PermissionFeature::DeviceInfo => "читать сведения об устройстве",
        PermissionFeature::BackgroundSync => "синхронизироваться в фоне",
        PermissionFeature::Bluetooth => "использовать Bluetooth",
        PermissionFeature::PersistentStorage => "хранить данные без ограничений по времени",
        PermissionFeature::ScreenWakeLock(_) => "не давать экрану гаснуть",
        PermissionFeature::Gamepad => "использовать геймпады",
    }
    .to_owned()
}

fn context_label(action: ContextMenuAction) -> String {
    // Подписи Servo английские — подставляем русские по действию.
    match action {
        ContextMenuAction::GoBack => "Назад",
        ContextMenuAction::GoForward => "Вперёд",
        ContextMenuAction::Reload => "Перезагрузить",
        ContextMenuAction::CopyLink => "Копировать адрес ссылки",
        ContextMenuAction::OpenLinkInNewWebView => "Открыть ссылку в новой вкладке",
        ContextMenuAction::CopyImageLink => "Копировать адрес изображения",
        ContextMenuAction::OpenImageInNewView => "Открыть изображение в новой вкладке",
        ContextMenuAction::Cut => "Вырезать",
        ContextMenuAction::Copy => "Копировать",
        ContextMenuAction::Paste => "Вставить",
        ContextMenuAction::SelectAll => "Выделить всё",
    }
    .to_owned()
}

/// Нарисовать модальное окно. `true` — запрос разрешён (ответ отправлен), окно можно убрать.
pub fn show(ctx: &egui::Context, modal: &mut Modal) -> bool {
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let mut finished = false;

    match modal {
        Modal::Dialog {
            origin,
            dialog,
            input,
        } => {
            let Some(current) = dialog.as_ref() else {
                return true;
            };
            let is_prompt = matches!(current, SimpleDialog::Prompt(_));
            let is_confirm = matches!(current, SimpleDialog::Confirm(_));
            let message = current.message().to_owned();
            let (mut ok, mut cancel) = (false, escape);
            egui::Window::new(format!("Страница {origin} сообщает"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
                .show(ctx, |ui| {
                    ui.set_max_width(460.0);
                    ui.label(&message);
                    if is_prompt {
                        let edit = ui.text_edit_singleline(input);
                        edit.request_focus();
                        if enter {
                            ok = true;
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || (enter && !is_prompt) {
                            ok = true;
                        }
                        if (is_confirm || is_prompt) && ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if ok || cancel {
                if let Some(dialog) = dialog.take() {
                    match (dialog, ok) {
                        (SimpleDialog::Prompt(mut prompt), true) => {
                            prompt.set_current_value(input);
                            prompt.confirm();
                        }
                        (dialog, true) => dialog.confirm(),
                        (dialog, false) => dialog.dismiss(),
                    }
                }
                finished = true;
            }
        }
        Modal::Permission {
            origin,
            feature,
            request,
        } => {
            let (mut allow, mut deny) = (false, escape);
            egui::Window::new("Запрос разрешения")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
                .show(ctx, |ui| {
                    ui.label(format!("Сайт {origin} хочет {}.", feature_name(*feature)));
                    ui.horizontal(|ui| {
                        if ui.button("Разрешить").clicked() {
                            allow = true;
                        }
                        if ui.button("Запретить").clicked() {
                            deny = true;
                        }
                    });
                });
            if allow || deny {
                if let Some(request) = request.take() {
                    if allow {
                        request.allow();
                    } else {
                        request.deny();
                    }
                }
                finished = true;
            }
        }
        Modal::Auth {
            url,
            for_proxy,
            request,
            user,
            pass,
        } => {
            let (mut login, mut cancel) = (false, escape);
            egui::Window::new("Требуется авторизация")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
                .show(ctx, |ui| {
                    ui.label(if *for_proxy {
                        format!("Прокси запрашивает логин и пароль для {url}")
                    } else {
                        format!("{url} запрашивает логин и пароль")
                    });
                    ui.horizontal(|ui| {
                        ui.label("Логин");
                        let edit = ui.text_edit_singleline(user);
                        // Фокус на первое поле, пока в окне ничего не выбрано.
                        if ui.memory(|m| m.focused().is_none()) {
                            edit.request_focus();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Пароль");
                        ui.add(egui::TextEdit::singleline(pass).password(true));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Войти").clicked() || enter {
                            login = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if login || cancel {
                if let Some(request) = request.take()
                    && login
                {
                    request.authenticate(std::mem::take(user), std::mem::take(pass));
                }
                finished = true;
            }
        }
        Modal::Select {
            element,
            selected,
            anchor,
        } => {
            let Some(current) = element.as_ref() else {
                return true;
            };
            let multiple = current.allow_select_multiple();
            let groups: Vec<OptionGroup> = current
                .options()
                .iter()
                .map(|group| match group {
                    SelectElementOptionOrOptgroup::Option(option) => (
                        None,
                        vec![(option.id, option.label.clone(), option.is_disabled)],
                    ),
                    SelectElementOptionOrOptgroup::Optgroup { label, options } => (
                        Some(label.clone()),
                        options
                            .iter()
                            .map(|o| (o.id, o.label.clone(), o.is_disabled))
                            .collect(),
                    ),
                })
                .collect();
            let mut pick: Option<usize> = None;
            let mut done = escape;
            let area = egui::Area::new(egui::Id::new("select-popup"))
                .fixed_pos(*anchor)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(180.0);
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for (label, options) in &groups {
                                    if let Some(label) = label {
                                        ui.label(egui::RichText::new(label).strong());
                                    }
                                    for (id, text, disabled) in options {
                                        let on = selected.contains(id);
                                        let response = ui
                                            .add_enabled_ui(!disabled, |ui| {
                                                ui.selectable_label(on, text)
                                            })
                                            .inner;
                                        if response.clicked() {
                                            pick = Some(*id);
                                        }
                                    }
                                }
                            });
                        if multiple && ui.button("Готово").clicked() {
                            done = true;
                        }
                    });
                });
            let clicked_outside = ctx.input(|i| i.pointer.any_click())
                && !ctx
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|p| area.response.rect.contains(p));
            if let Some(id) = pick {
                if multiple {
                    if let Some(position) = selected.iter().position(|s| *s == id) {
                        selected.remove(position);
                    } else {
                        selected.push(id);
                    }
                } else {
                    *selected = vec![id];
                    done = true;
                }
            }
            if done || clicked_outside {
                if let Some(mut element) = element.take() {
                    element.select(selected.clone());
                    element.submit();
                }
                finished = true;
            }
        }
        Modal::Context { menu, anchor } => {
            let Some(current) = menu.as_ref() else {
                return true;
            };
            let items: Vec<(String, ContextMenuAction, bool)> = current
                .items()
                .iter()
                .filter_map(|item| match item {
                    ContextMenuItem::Item {
                        label: _,
                        action,
                        enabled,
                    } => Some((context_label(*action), *action, *enabled)),
                    ContextMenuItem::Separator => None,
                })
                .collect();
            let mut chosen: Option<ContextMenuAction> = None;
            let area = egui::Area::new(egui::Id::new("context-menu"))
                .fixed_pos(*anchor)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for (label, action, enabled) in &items {
                            if ui
                                .add_enabled(*enabled, egui::Button::new(label).frame(false))
                                .clicked()
                            {
                                chosen = Some(*action);
                            }
                        }
                    });
                });
            let clicked_outside = ctx.input(|i| i.pointer.any_click())
                && !ctx
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|p| area.response.rect.contains(p));
            if chosen.is_some() || escape || clicked_outside {
                if let Some(menu) = menu.take() {
                    match chosen {
                        Some(action) => menu.select(action),
                        None => menu.dismiss(),
                    }
                }
                finished = true;
            }
        }
    }
    finished
}
