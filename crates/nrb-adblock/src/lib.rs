//! Блокировщик рекламы и трекеров (I12): правила Adblock Plus (EasyList, EasyPrivacy, uBlock-списки) поверх крейта `adblock`.
//! Решение принимается до сети — `Blocker::check` зовётся из хука Servo `load_web_resource` и из режима I (`nrb-net`).
//! Списки в репозиторий не вшиваются (лицензии EasyList — GPLv3/CC BY-SA): пользователь кладёт `*.txt` в каталог фильтров
//! профиля или скачивает их командой `nrb adblock update`.

use std::path::Path;
use std::sync::Mutex;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;

/// Тип ресурса для правил с `$script`, `$image`, `$third-party` и т. п.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Document,
    Subdocument,
    Script,
    Stylesheet,
    Image,
    Font,
    Media,
    Xhr,
    Websocket,
    Ping,
    Other,
}

impl ResourceKind {
    /// По имени варианта `Destination` запроса Servo (`format!("{:?}")`): отдельная зависимость от типа не нужна.
    pub fn from_destination(name: &str) -> Self {
        match name {
            "Document" => Self::Document,
            "Frame" | "IFrame" | "Embed" | "Object" => Self::Subdocument,
            "Script" | "Worker" | "SharedWorker" | "ServiceWorker" | "AudioWorklet"
            | "PaintWorklet" => Self::Script,
            "Style" | "Xslt" => Self::Stylesheet,
            "Image" => Self::Image,
            "Font" => Self::Font,
            "Audio" | "Video" | "Track" => Self::Media,
            "Report" => Self::Ping,
            // `fetch()`/XHR приходят без назначения
            "None" | "Json" | "Text" | "Manifest" | "WebIdentity" => Self::Xhr,
            _ => Self::Other,
        }
    }

    fn as_adblock(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Subdocument => "subdocument",
            Self::Script => "script",
            Self::Stylesheet => "stylesheet",
            Self::Image => "image",
            Self::Font => "font",
            Self::Media => "media",
            Self::Xhr => "xmlhttprequest",
            Self::Websocket => "websocket",
            Self::Ping => "ping",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block,
}

pub struct Blocker {
    // `Engine` не `Sync` (кэш регулярных выражений), поэтому за мьютексом.
    engine: Mutex<Engine>,
    rules: usize,
}

impl Blocker {
    /// Блокировщик без правил: пропускает всё.
    pub fn empty() -> Self {
        Self::from_lists(std::iter::empty::<&str>())
    }

    /// Собирает движок из текстов списков (по правилу на строку; комментарии и неподдерживаемые строки пропускаются).
    pub fn from_lists<I, S>(lists: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = FilterSet::new(false);
        let mut rules = 0usize;
        for list in lists {
            let text = list.as_ref();
            rules += text
                .lines()
                .filter(|l| {
                    let l = l.trim();
                    !l.is_empty() && !l.starts_with('!') && !l.starts_with('[')
                })
                .count();
            set.add_filter_list(text.to_owned(), ParseOptions::default());
        }
        Self {
            engine: Mutex::new(Engine::new_with_filter_set(set)),
            rules,
        }
    }

    /// Все `*.txt` из каталога (порядок по имени). Нет каталога — пустой блокировщик.
    pub fn from_dir(dir: &Path) -> std::io::Result<Self> {
        let mut files: Vec<_> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"))
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        files.sort();
        let texts = files
            .iter()
            .map(std::fs::read_to_string)
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(Self::from_lists(texts))
    }

    /// Сколько строк-правил загружено (грубо: без комментариев).
    pub fn rule_count(&self) -> usize {
        self.rules
    }

    /// `source` — адрес страницы верхнего уровня (определяет `$third-party` и `$domain=`). Главный документ не блокируется
    /// никогда: заблокированная страница вместо рекламы стала бы пустым окном без объяснений.
    pub fn check(&self, url: &str, source: &str, kind: ResourceKind) -> Verdict {
        if self.rules == 0 || kind == ResourceKind::Document {
            return Verdict::Allow;
        }
        let Ok(request) = Request::new(url, source, kind.as_adblock(), "get") else {
            return Verdict::Allow;
        };
        let result = self
            .engine
            .lock()
            .expect("mutex блокировщика")
            .check_network_request(&request);
        if result.should_block() {
            Verdict::Block
        } else {
            Verdict::Allow
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "\
! тестовый список
[Adblock Plus 2.0]
||ads.example.com^
||tracker.test^$third-party
/banner/*$image
@@||ads.example.com/allowed^
";

    fn blocker() -> Blocker {
        Blocker::from_lists([LIST])
    }

    #[test]
    fn blocks_by_domain_and_respects_exceptions() {
        let b = blocker();
        let page = "https://news.test/article";
        assert_eq!(
            b.check("https://ads.example.com/x.js", page, ResourceKind::Script),
            Verdict::Block
        );
        assert_eq!(
            b.check(
                "https://ads.example.com/allowed/x.js",
                page,
                ResourceKind::Script
            ),
            Verdict::Allow,
            "правило-исключение @@ снимает блокировку"
        );
        assert_eq!(
            b.check("https://news.test/app.js", page, ResourceKind::Script),
            Verdict::Allow
        );
    }

    #[test]
    fn third_party_option_and_resource_type_matter() {
        let b = blocker();
        assert_eq!(
            b.check(
                "https://tracker.test/p.gif",
                "https://news.test/",
                ResourceKind::Image
            ),
            Verdict::Block
        );
        assert_eq!(
            b.check(
                "https://tracker.test/p.gif",
                "https://tracker.test/home",
                ResourceKind::Image
            ),
            Verdict::Allow,
            "свой домен — не третья сторона"
        );
        assert_eq!(
            b.check(
                "https://x.test/banner/1.png",
                "https://y.test/",
                ResourceKind::Image
            ),
            Verdict::Block
        );
        assert_eq!(
            b.check(
                "https://x.test/banner/1.html",
                "https://y.test/",
                ResourceKind::Subdocument
            ),
            Verdict::Allow,
            "правило `$image` не действует на документы"
        );
    }

    #[test]
    fn main_document_is_never_blocked() {
        let b = blocker();
        assert_eq!(
            b.check(
                "https://ads.example.com/",
                "https://ads.example.com/",
                ResourceKind::Document
            ),
            Verdict::Allow
        );
    }

    #[test]
    fn servo_destinations_map_to_filter_types() {
        assert_eq!(
            ResourceKind::from_destination("Script"),
            ResourceKind::Script
        );
        assert_eq!(ResourceKind::from_destination("None"), ResourceKind::Xhr);
        assert_eq!(
            ResourceKind::from_destination("IFrame"),
            ResourceKind::Subdocument
        );
        assert_eq!(
            ResourceKind::from_destination("что-то новое"),
            ResourceKind::Other
        );
    }

    #[test]
    fn empty_blocker_allows_everything_and_bad_urls_do_not_panic() {
        let b = Blocker::empty();
        assert_eq!(b.rule_count(), 0);
        assert_eq!(
            b.check("https://a.test/", "", ResourceKind::Script),
            Verdict::Allow
        );
        let b = blocker();
        assert_eq!(
            b.check("не url", "тоже", ResourceKind::Script),
            Verdict::Allow
        );
        assert_eq!(b.rule_count(), 4);
    }

    #[test]
    fn lists_load_from_a_directory() {
        let dir = std::env::temp_dir().join(format!("nrb-adblock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "||ads.example.com^\n").unwrap();
        std::fs::write(dir.join("ignored.md"), "||nothing.test^\n").unwrap();
        let b = Blocker::from_dir(&dir).unwrap();
        assert_eq!(b.rule_count(), 1);
        assert_eq!(
            b.check(
                "https://ads.example.com/a.js",
                "https://x.test/",
                ResourceKind::Script
            ),
            Verdict::Block
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            Blocker::from_dir(&dir).unwrap().rule_count(),
            0,
            "нет каталога — нет правил"
        );
    }
}
