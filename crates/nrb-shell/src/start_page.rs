//! Страница новой вкладки: тёмный/светлый фон в цвет панели инструментов вместо белого `about:blank`. Это `data:`-адрес
//! (ни сети, ни файлов); в адресной строке, истории и сессии он выглядит как пустая вкладка.

use url::Url;

use crate::theme::Palette;

const MARKER: &str = "nrb-start-page";

fn hex(c: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Адрес страницы новой вкладки в цветах палитры.
pub fn url(p: &Palette) -> Url {
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Новая вкладка</title><!--{MARKER}-->\
<style>html,body{{height:100%;margin:0;background:{bg};color:{text};font:16px system-ui,'Segoe UI',Ubuntu,sans-serif}}\
body{{display:flex;align-items:center;justify-content:center;flex-direction:column;user-select:none}}\
h1{{font-size:30px;letter-spacing:.42em;margin:0;padding-left:.42em;font-weight:600}}\
h1 b{{color:{accent};font-weight:600}}p{{color:{muted};margin:14px 0 0;font-size:15px}}\
kbd{{background:{field};border:1px solid {border};border-radius:5px;padding:1px 7px;font:13px ui-monospace,Consolas,monospace;color:{text}}}</style>\
<h1>NET<b>RUNNER</b></h1><p>Введите адрес или запрос в строке сверху</p>\
<p><kbd>Ctrl</kbd>+<kbd>L</kbd> адресная строка &nbsp;·&nbsp; <kbd>Ctrl</kbd>+<kbd>T</kbd> новая вкладка</p>",
        bg = hex(p.surface),
        text = hex(p.text),
        muted = hex(p.muted),
        accent = hex(p.accent),
        field = hex(p.field),
        border = hex(p.border),
    );
    let mut encoded = String::from("data:text/html;charset=utf-8,");
    for byte in html.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    Url::parse(&encoded).expect("валидный data-адрес")
}

/// Это страница новой вкладки (или пустая вкладка)?
pub fn is_blank(url: &str) -> bool {
    url == "about:blank"
        || (url.starts_with("data:text/html;charset=utf-8,") && url.contains(MARKER))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::palette;

    #[test]
    fn start_page_is_recognised_in_both_themes() {
        for theme in [nrb_profile::Theme::Dark, nrb_profile::Theme::Light] {
            let url = url(&palette(theme));
            assert!(is_blank(url.as_str()));
        }
        assert!(is_blank("about:blank"));
        assert!(!is_blank("https://example.org/"));
    }
}
