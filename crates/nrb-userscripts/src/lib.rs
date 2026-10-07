//! Пользовательские скрипты (`*.user.js`) и стили (`*.user.css`) — подмножество формата Greasemonkey/Tampermonkey (I12).
//!
//! Заголовок `// ==UserScript==` … `// ==/UserScript==` (для стилей — в блочном комментарии `/* ==UserStyle== … */`):
//! `@name`, `@match`/`@include` (шаблоны вида `*://*.example.org/path/*`), `@exclude`, `@run-at` (`document-start` |
//! `document-end`, по умолчанию конец документа). Без `@match` скрипт **не** запускается нигде — иначе забытый заголовок
//! превратил бы скрипт в выполняющийся на каждой странице.
//!
//! Servo внедряет пользовательские скрипты во все страницы, поэтому фильтр по адресу — часть самого внедряемого кода:
//! `wrap` возвращает JavaScript, который проверяет `location.href` по регулярным выражениям и только затем выполняет тело.
//! API Greasemonkey (`GM_*`) не поддерживается.

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunAt {
    DocumentStart,
    DocumentEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Script,
    Style,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserContent {
    pub name: String,
    pub kind: Kind,
    pub matches: Vec<String>,
    pub excludes: Vec<String>,
    pub run_at: RunAt,
    pub body: String,
}

/// Шаблон `@match` → тело регулярного выражения (синтаксис общий для JS и Rust `regex`), привязанное к началу и концу.
/// `*` — любая последовательность символов, `*://` — http или https; остальное — литералы.
pub fn pattern_to_regex(pattern: &str) -> String {
    let mut out = String::from("^");
    let rest = if let Some(rest) = pattern.strip_prefix("*://") {
        out.push_str("https?://");
        rest
    } else {
        pattern
    };
    for c in rest.chars() {
        match c {
            '*' => out.push_str(".*"),
            c if "\\^$.|?+()[]{}/".contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    out
}

fn header_block(text: &str, kind: Kind) -> Option<&str> {
    let (open, close) = match kind {
        Kind::Script => ("==UserScript==", "==/UserScript=="),
        Kind::Style => ("==UserStyle==", "==/UserStyle=="),
    };
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

impl UserContent {
    pub fn parse(name: &str, text: &str, kind: Kind) -> Self {
        let mut content = Self {
            name: name.to_owned(),
            kind,
            matches: Vec::new(),
            excludes: Vec::new(),
            run_at: RunAt::DocumentEnd,
            body: text.to_owned(),
        };
        let Some(block) = header_block(text, kind) else {
            return content;
        };
        if kind == Kind::Style {
            // Заголовок стиля лежит в блочном комментарии, а шаблоны вида `…:*/*` содержат `*/`: тело — всё, что идёт после
            // закрывающего `==/UserStyle==` и первого следующего за ним `*/`, иначе «хвост» заголовка испортил бы первое правило.
            if let Some(close) = text.find("==/UserStyle==") {
                let after = &text[close + "==/UserStyle==".len()..];
                content.body = after
                    .find("*/")
                    .map_or(after, |end| &after[end + 2..])
                    .trim_start()
                    .to_owned();
            }
        }
        for line in block.lines() {
            let line = line
                .trim()
                .trim_start_matches("//")
                .trim_start_matches('*')
                .trim();
            let Some(rest) = line.strip_prefix('@') else {
                continue;
            };
            let (key, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let value = value.trim();
            match key {
                "name" if !value.is_empty() => content.name = value.to_owned(),
                "match" | "include" if !value.is_empty() => content.matches.push(value.to_owned()),
                "exclude" | "exclude-match" if !value.is_empty() => {
                    content.excludes.push(value.to_owned())
                }
                "run-at" => {
                    content.run_at = if value == "document-start" {
                        RunAt::DocumentStart
                    } else {
                        RunAt::DocumentEnd
                    }
                }
                _ => {}
            }
        }
        content
    }

    /// JavaScript для внедрения: проверка адреса + тело (стиль — вставка `<style>`).
    /// Без `@match` возвращает `None` (не запускать нигде).
    pub fn wrap(&self) -> Option<String> {
        if self.matches.is_empty() {
            return None;
        }
        let list = |items: &[String]| {
            items
                .iter()
                .map(|p| js_string(&pattern_to_regex(p)))
                .collect::<Vec<_>>()
                .join(",")
        };
        let action = match self.kind {
            Kind::Script => self.body.clone(),
            Kind::Style => format!(
                "var s=document.createElement('style');s.textContent={};(document.head||document.documentElement).appendChild(s);",
                js_string(&self.body)
            ),
        };
        let run = match self.run_at {
            RunAt::DocumentStart => format!("(function(){{{action}\n}})();"),
            RunAt::DocumentEnd => format!(
                "var run=function(){{{action}\n}};if(document.readyState==='loading'){{document.addEventListener('DOMContentLoaded',run);}}else{{run();}}"
            ),
        };
        Some(format!(
            "(function(){{var u=location.href;var ok=function(l){{return l.some(function(s){{return new RegExp(s).test(u);}});}};\
if(!ok([{}])||ok([{}]))return;{run}}})();",
            list(&self.matches),
            list(&self.excludes)
        ))
    }
}

/// Строка JS в двойных кавычках (с экранированием, безопасная и для `</script>`-подобных последовательностей).
fn js_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '<' => out.push_str("\\u003c"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Читает `*.user.js` и `*.user.css` из каталога (по имени файла); нет каталога — пусто.
pub fn load_dir(dir: &Path) -> Result<Vec<UserContent>, Error> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Io(dir.display().to_string(), e)),
    };
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let file = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let kind = if file.ends_with(".user.js") {
            Kind::Script
        } else if file.ends_with(".user.css") {
            Kind::Style
        } else {
            continue;
        };
        let text =
            std::fs::read_to_string(&path).map_err(|e| Error::Io(path.display().to_string(), e))?;
        out.push(UserContent::parse(file, &text, kind));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn matches(pattern: &str, url: &str) -> bool {
        Regex::new(&pattern_to_regex(pattern))
            .unwrap()
            .is_match(url)
    }

    #[test]
    fn match_patterns_behave_like_greasemonkey() {
        assert!(matches("*://example.org/*", "https://example.org/a/b?c=1"));
        assert!(matches("*://example.org/*", "http://example.org/"));
        assert!(!matches(
            "*://example.org/*",
            "https://evil.com/?u=https://example.org/"
        ));
        assert!(!matches("*://example.org/*", "https://notexample.org/x"));
        assert!(matches(
            "https://*.example.org/*",
            "https://www.example.org/x"
        ));
        assert!(!matches(
            "https://*.example.org/*",
            "http://www.example.org/x"
        ));
        assert!(matches(
            "http://127.0.0.1:*/*",
            "http://127.0.0.1:8080/page.html"
        ));
        assert!(matches(
            "https://example.org/path",
            "https://example.org/path"
        ));
        assert!(!matches(
            "https://example.org/path",
            "https://example.org/path/more"
        ));
        assert!(
            !matches("https://example.org/a.b", "https://example.org/aXb"),
            "точка — литерал"
        );
    }

    #[test]
    fn header_is_parsed() {
        let text = "// ==UserScript==\n// @name Тест\n// @match *://a.test/*\n// @match https://b.test/*\n// @exclude *://a.test/private/*\n// @run-at document-start\n// ==/UserScript==\nconsole.log(1);";
        let c = UserContent::parse("x.user.js", text, Kind::Script);
        assert_eq!(c.name, "Тест");
        assert_eq!(c.matches.len(), 2);
        assert_eq!(c.excludes, vec!["*://a.test/private/*"]);
        assert_eq!(c.run_at, RunAt::DocumentStart);
    }

    #[test]
    fn without_match_nothing_runs() {
        let c = UserContent::parse("x.user.js", "alert(1)", Kind::Script);
        assert!(c.wrap().is_none());
    }

    #[test]
    fn wrapper_contains_address_checks_and_body() {
        let c = UserContent::parse(
            "x.user.js",
            "// ==UserScript==\n// @match *://a.test/*\n// ==/UserScript==\ndocument.title='x';",
            Kind::Script,
        );
        let js = c.wrap().unwrap();
        assert!(js.contains("location.href"));
        assert!(js.contains("^https?://a\\\\.test"), "{js}");
        assert!(js.contains("DOMContentLoaded"));
        assert!(js.contains("document.title='x';"));
    }

    #[test]
    fn styles_become_style_elements_and_escape_markup() {
        let c = UserContent::parse(
            "s.user.css",
            "/* ==UserStyle==\n @match *://a.test/*\n ==/UserStyle== */\nh1 { color: red; }\n/* </style><script> */",
            Kind::Style,
        );
        let js = c.wrap().unwrap();
        assert!(js.contains("createElement('style')"));
        assert!(!js.contains("</style>"), "закрывающие теги экранированы");
        assert!(
            !js.contains("UserStyle"),
            "заголовок в тело стиля не попадает"
        );
    }

    #[test]
    fn style_header_with_comment_terminator_inside_patterns_does_not_leak_into_css() {
        let c = UserContent::parse(
            "s.user.css",
            "/* ==UserStyle==\n @match http://127.0.0.1:*/*\n ==/UserStyle== */\nh1 { color: red; }",
            Kind::Style,
        );
        assert_eq!(c.body, "h1 { color: red; }");
        assert_eq!(c.matches, vec!["http://127.0.0.1:*/*"]);
    }

    #[test]
    fn directory_loading_filters_by_suffix() {
        let dir = std::env::temp_dir().join(format!("nrb-us-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.user.js"), "// @match *://x/*\n").unwrap();
        std::fs::write(dir.join("b.user.css"), "x{}").unwrap();
        std::fs::write(dir.join("readme.txt"), "нет").unwrap();
        let all = load_dir(&dir).unwrap();
        assert_eq!(all.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(load_dir(&dir).unwrap().is_empty());
    }
}
