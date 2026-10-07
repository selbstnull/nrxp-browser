use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// Настройки пользователя (`settings.toml` в каталоге профиля). Неизвестные поля игнорируются, отсутствующие — по умолчанию.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Шаблон поиска с `{}` на месте запроса.
    pub search: String,
    /// Страница новой вкладки; пусто — пустая.
    pub homepage: String,
    pub theme: Theme,
    /// Масштаб страниц по умолчанию (1.0 = 100%).
    pub default_zoom: f32,
    /// Восстанавливать вкладки при запуске без аргументов.
    pub restore_session: bool,
    /// Записывать историю посещений.
    pub record_history: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            search: "https://duckduckgo.com/?q={}".into(),
            homepage: String::new(),
            theme: Theme::Dark,
            default_zoom: 1.0,
            restore_session: true,
            record_history: true,
        }
    }
}

/// Готовые поисковики для окна настроек.
pub const SEARCH_ENGINES: &[(&str, &str)] = &[
    ("DuckDuckGo", "https://duckduckgo.com/?q={}"),
    ("Google", "https://www.google.com/search?q={}"),
    ("Bing", "https://www.bing.com/search?q={}"),
    ("Яндекс", "https://yandex.ru/search/?text={}"),
    ("Brave", "https://search.brave.com/search?q={}"),
];

impl Settings {
    /// Читает настройки; нет файла — значения по умолчанию; битый файл — ошибка (файл не затирается).
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("settings.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str(&text).map_err(|e| Error::Format(format!("{}: {e}", path.display())))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
        .map(Self::sanitized)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| Error::Format(e.to_string()))?;
        // Запись через временный файл: сбой посреди записи не оставит полфайла.
        let tmp = dir.join("settings.toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, dir.join("settings.toml"))?;
        Ok(())
    }

    /// Приводит значения к допустимым (масштаб, шаблон поиска).
    fn sanitized(mut self) -> Self {
        self.default_zoom = self.default_zoom.clamp(0.3, 5.0);
        if !self.search.contains("{}") {
            self.search = Self::default().search;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()).unwrap(), Settings::default());
    }

    #[test]
    fn roundtrip_and_sanitize() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            theme: Theme::Light,
            default_zoom: 1.25,
            homepage: "https://example.org".into(),
            ..Settings::default()
        };
        settings.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()).unwrap(), settings);

        std::fs::write(
            dir.path().join("settings.toml"),
            "default_zoom = 99.0\nsearch = \"https://x/\"\nunknown = 1\n",
        )
        .unwrap();
        let loaded = Settings::load(dir.path()).unwrap();
        assert_eq!(loaded.default_zoom, 5.0);
        assert_eq!(
            loaded.search,
            Settings::default().search,
            "шаблон без {{}} заменяется по умолчанию"
        );
    }

    #[test]
    fn broken_file_is_an_error_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "это не toml ===").unwrap();
        assert!(Settings::load(dir.path()).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "это не toml ===");
    }
}
