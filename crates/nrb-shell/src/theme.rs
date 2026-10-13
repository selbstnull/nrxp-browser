//! Оформление оболочки: палитра приложения Netrunner (те же токены, что в `netrunner-app/style.css`, переведённые из oklch в
//! sRGB) в строгой раскладке, как у Firefox: тёмная полоса вкладок, активная вкладка сливается с панелью инструментов, адресная
//! строка — скруглённое поле. Акцентный цвет (бирюзовый) используется скупо: фокус, индикатор загрузки, выделение.

use egui::{Color32, CornerRadius, Shadow, Stroke};

/// Цвета интерфейса. Читаются из контекста (`current`), чтобы окна и панели рисовались в той же теме.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Фон полосы вкладок (самый тёмный/светлый слой).
    pub strip: Color32,
    /// Активная вкладка, панель инструментов, окна и меню.
    pub surface: Color32,
    /// Поля ввода и адресная строка.
    pub field: Color32,
    /// Наведение на кнопки и неактивные вкладки.
    pub hover: Color32,
    /// Тонкие разделители и рамки.
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    /// Цвет текста на акценте.
    pub on_accent: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub ok: Color32,
}

pub fn palette(theme: nrb_profile::Theme) -> Palette {
    match theme {
        // Токены `.dark` из netrunner-app/style.css: background / card / muted / foreground / muted-foreground / primary.
        nrb_profile::Theme::Dark => Palette {
            dark: true,
            strip: Color32::from_rgb(9, 11, 12),
            surface: Color32::from_rgb(22, 27, 29),
            field: Color32::from_rgb(34, 41, 43),
            hover: Color32::from_rgb(45, 54, 57),
            border: Color32::from_rgb(44, 51, 53),
            text: Color32::from_rgb(249, 251, 251),
            muted: Color32::from_rgb(156, 168, 171),
            accent: Color32::from_rgb(0, 236, 227),
            on_accent: Color32::from_rgb(9, 11, 12),
            danger: Color32::from_rgb(255, 100, 103),
            warn: Color32::from_rgb(250, 200, 50),
            ok: Color32::from_rgb(70, 214, 130),
        },
        // Токены `:root`.
        nrb_profile::Theme::Light => Palette {
            dark: false,
            strip: Color32::from_rgb(232, 236, 237),
            surface: Color32::from_rgb(255, 255, 255),
            field: Color32::from_rgb(241, 243, 243),
            hover: Color32::from_rgb(222, 227, 228),
            border: Color32::from_rgb(214, 220, 221),
            text: Color32::from_rgb(9, 11, 12),
            muted: Color32::from_rgb(103, 120, 124),
            accent: Color32::from_rgb(0, 131, 173),
            on_accent: Color32::from_rgb(255, 255, 255),
            danger: Color32::from_rgb(231, 0, 11),
            warn: Color32::from_rgb(190, 130, 0),
            ok: Color32::from_rgb(20, 150, 80),
        },
    }
}

fn palette_id() -> egui::Id {
    egui::Id::new("nrb-palette")
}

/// Палитра, установленная `apply` (тёмная — если ещё не устанавливали).
pub fn current(ctx: &egui::Context) -> Palette {
    ctx.data(|d| d.get_temp::<Palette>(palette_id()))
        .unwrap_or_else(|| palette(nrb_profile::Theme::Dark))
}

/// Устанавливает палитру и визуальный стиль egui для всех окон.
pub fn apply(ctx: &egui::Context, theme: nrb_profile::Theme) {
    let p = palette(theme);
    ctx.data_mut(|d| d.insert_temp(palette_id(), p));

    let mut v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.surface;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 150 } else { 50 }),
    };
    v.popup_shadow = v.window_shadow;
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.field;
    v.code_bg_color = p.field;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(
        p.accent.r(),
        p.accent.g(),
        p.accent.b(),
        if p.dark { 70 } else { 55 },
    );
    v.selection.stroke = Stroke::new(1.0, p.accent);

    let radius = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = p.field;
    w.inactive.weak_bg_fill = p.field;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    let pressed = Color32::from_rgba_unmultiplied(
        p.accent.r(),
        p.accent.g(),
        p.accent.b(),
        if p.dark { 60 } else { 45 },
    );
    w.active.bg_fill = pressed;
    w.active.weak_bg_fill = pressed;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.hover;
    w.open.weak_bg_fill = p.hover;
    w.open.bg_stroke = Stroke::new(1.0, p.border);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = radius;

    ctx.set_visuals(v);
    ctx.global_style_mut(|style| {
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.window_margin = egui::Margin::same(14);
        style.spacing.interact_size.y = 26.0;
    });
}
