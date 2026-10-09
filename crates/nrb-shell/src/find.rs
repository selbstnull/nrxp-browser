//! Поиск по странице (N5a): Ctrl+F. Servo 0.7 не даёт API поиска, поэтому вся работа — скриптом в странице (`evaluate_javascript`):
//! обход текстовых узлов, подсветка `<mark>`, переход к совпадению, очистка. Ограничения (см. `PLAN.md` форка, N5b — поиск в движке):
//! скрипт меняет DOM страницы (метки убираются при закрытии панели), не заходит в shadow DOM и чужие iframe, совпадение не
//! может пересекать границу элементов, не ищет в скрытом тексте, ограничение 5000 совпадений.

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

pub const HELPER: &str = r#"(function () {
  if (window.__nrbFind) return;
  var MARK = 'nrb-find-mark', ACTIVE = 'nrb-find-active', LIMIT = 5000, marks = [], active = -1;
  function style() {
    if (document.getElementById('__nrb_find_style')) return;
    var s = document.createElement('style'); s.id = '__nrb_find_style';
    s.textContent = 'mark.' + MARK + '{background:#ffe066;color:#000}mark.' + ACTIVE + '{background:#ff9f43;box-shadow:0 0 0 2px #ff9f43}';
    (document.head || document.documentElement).appendChild(s);
  }
  function clear() {
    marks.forEach(function (m) {
      var p = m.parentNode; if (!p) return;
      p.replaceChild(document.createTextNode(m.textContent), m); p.normalize();
    });
    marks = []; active = -1;
  }
  function state() { return { count: marks.length, active: marks.length ? active : -1 }; }
  function visible(el, cache) {
    if (cache.has(el)) return cache.get(el);
    var ok = !!(el.getClientRects && el.getClientRects().length);
    cache.set(el, ok); return ok;
  }
  function search(q, caseSensitive) {
    clear();
    if (!q || !document.body) return state();
    style();
    var needle = caseSensitive ? q : q.toLowerCase(), nodes = [], cache = new Map();
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, null);
    for (var n = walker.nextNode(); n; n = walker.nextNode()) {
      var p = n.parentNode;
      if (!p || !n.nodeValue) continue;
      var tag = p.nodeName;
      if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEXTAREA' || tag === 'TITLE') continue;
      if (!visible(p, cache)) continue;
      nodes.push(n);
    }
    for (var i = 0; i < nodes.length && marks.length < LIMIT; i++) {
      var node = nodes[i], text = node.nodeValue, hay = caseSensitive ? text : text.toLowerCase();
      var at = hay.indexOf(needle); if (at < 0) continue;
      var frag = document.createDocumentFragment(), last = 0;
      while (at >= 0 && marks.length < LIMIT) {
        if (at > last) frag.appendChild(document.createTextNode(text.slice(last, at)));
        var m = document.createElement('mark'); m.className = MARK; m.textContent = text.slice(at, at + needle.length);
        frag.appendChild(m); marks.push(m);
        last = at + needle.length; at = hay.indexOf(needle, last);
      }
      if (last < text.length) frag.appendChild(document.createTextNode(text.slice(last)));
      node.parentNode.replaceChild(frag, node);
    }
    if (marks.length) { active = 0; marks[0].classList.add(ACTIVE); marks[0].scrollIntoView({ block: 'center' }); }
    return state();
  }
  function step(d) {
    if (!marks.length) return state();
    marks[active].classList.remove(ACTIVE);
    active = (active + d + marks.length) % marks.length;
    marks[active].classList.add(ACTIVE); marks[active].scrollIntoView({ block: 'center' });
    return state();
  }
  window.__nrbFind = { search: search, step: step, clear: clear };
})();"#;

/// Строка JS-литерала (двойные кавычки) для произвольного текста запроса.
pub fn js_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn search_script(query: &str, case_sensitive: bool) -> String {
    format!(
        "{HELPER}\nJSON.stringify(window.__nrbFind.search({}, {}))",
        js_string(query),
        case_sensitive
    )
}

pub fn step_script(direction: i32) -> String {
    format!(
        "{HELPER}\nJSON.stringify(window.__nrbFind.step({}))",
        direction.signum()
    )
}

pub fn clear_script() -> String {
    format!("{HELPER}\nwindow.__nrbFind.clear(); 'ok'")
}

/// Разбор ответа скрипта `{"count":N,"active":I}`.
pub fn parse_result(json: &str) -> Option<FindResult> {
    let number = |key: &str| -> Option<i64> {
        let at = json.find(&format!("\"{key}\":"))? + key.len() + 3;
        let rest = &json[at..];
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        rest[..end].trim().parse().ok()
    };
    let count = number("count")?.max(0) as usize;
    let active = number("active")?;
    Some(FindResult {
        count,
        active: (active >= 0).then_some(active as usize),
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
            if ui.button("↓").on_hover_text("Следующее (Enter)").clicked() {
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
            if ui.button("✕").on_hover_text("Закрыть (Esc)").clicked()
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
    fn query_is_escaped_into_a_js_literal() {
        assert_eq!(js_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(js_string("</script>"), "\"\\u003c/script>\"");
        assert_eq!(js_string("строка\n"), "\"строка\\n\"");
        let script = search_script("x\"); alert(1); (\"", false);
        assert!(
            script.contains("search(\"x\\\"); alert(1); (\\\"\", false)"),
            "{script}"
        );
    }

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
        assert_eq!(parse_result("мусор"), None);
    }
}
