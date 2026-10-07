use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visits: u32,
    /// Unix-время последнего посещения, секунды.
    pub last_visit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub created: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabState {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub tabs: Vec<TabState>,
    pub active: usize,
}

/// Хранилище профиля: SQLite (`profile.sqlite`) для истории и закладок, `session.json` для вкладок.
pub struct Store {
    conn: Connection,
    dir: PathBuf,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Адреса, которые не стоит хранить в истории: внутренние страницы и данные.
fn recordable(url: &str) -> bool {
    Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https" | "file"))
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = Connection::open(dir.join("profile.sqlite"))?;
        Self::init(conn, dir)
    }

    /// Хранилище в памяти (приватный сеанс, тесты).
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, Path::new(""))
    }

    fn init(conn: Connection, dir: &Path) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS history (
                 url TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '',
                 visits INTEGER NOT NULL DEFAULT 1, last_visit INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS history_last ON history(last_visit DESC);
             CREATE TABLE IF NOT EXISTS bookmarks (
                 url TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', created INTEGER NOT NULL);",
        )?;
        Ok(Self {
            conn,
            dir: dir.to_path_buf(),
        })
    }

    // ---- история ----

    /// Фиксирует посещение: новая запись или `visits += 1`; непустой заголовок обновляется.
    pub fn record_visit(&self, url: &str, title: &str) -> Result<()> {
        if !recordable(url) {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO history(url, title, visits, last_visit) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(url) DO UPDATE SET visits = visits + 1, last_visit = ?3,
                 title = CASE WHEN ?2 <> '' THEN ?2 ELSE title END",
            params![url, title, now()],
        )?;
        Ok(())
    }

    /// Обновляет заголовок уже записанного адреса (заголовок часто приходит после загрузки).
    pub fn update_title(&self, url: &str, title: &str) -> Result<()> {
        if !title.is_empty() {
            self.conn.execute(
                "UPDATE history SET title = ?2 WHERE url = ?1",
                params![url, title],
            )?;
        }
        Ok(())
    }

    pub fn recent_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        self.query_history(
            "SELECT url, title, visits, last_visit FROM history ORDER BY last_visit DESC LIMIT ?1",
            params![limit as i64],
        )
    }

    /// Поиск по адресу и заголовку без учёта регистра (в т.ч. кириллицы: SQLite `LIKE` регистронезависим только для ASCII,
    /// поэтому фильтруем в Rust по `to_lowercase`). Чаще посещаемые и свежие — выше.
    pub fn search_history(&self, query: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
        const SCAN_LIMIT: i64 = 20_000;
        let needle = query.trim().to_lowercase();
        let candidates = self.query_history(
            "SELECT url, title, visits, last_visit FROM history ORDER BY visits DESC, last_visit DESC LIMIT ?1",
            params![SCAN_LIMIT],
        )?;
        Ok(candidates
            .into_iter()
            .filter(|entry| {
                entry.url.to_lowercase().contains(&needle)
                    || entry.title.to_lowercase().contains(&needle)
            })
            .take(limit)
            .collect())
    }

    fn query_history(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<HistoryEntry>> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map(params, |row| {
            Ok(HistoryEntry {
                url: row.get(0)?,
                title: row.get(1)?,
                visits: row.get(2)?,
                last_visit: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn delete_history_entry(&self, url: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM history WHERE url = ?1", params![url])?;
        Ok(())
    }

    pub fn clear_history(&self) -> Result<()> {
        self.conn.execute("DELETE FROM history", [])?;
        Ok(())
    }

    // ---- закладки ----

    pub fn is_bookmarked(&self, url: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM bookmarks WHERE url = ?1",
                params![url],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn add_bookmark(&self, url: &str, title: &str) -> Result<()> {
        if recordable(url) {
            self.conn.execute(
                "INSERT INTO bookmarks(url, title, created) VALUES (?1, ?2, ?3)
                 ON CONFLICT(url) DO UPDATE SET title = excluded.title",
                params![url, title, now()],
            )?;
        }
        Ok(())
    }

    pub fn remove_bookmark(&self, url: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM bookmarks WHERE url = ?1", params![url])?;
        Ok(())
    }

    /// Переключает закладку; возвращает новое состояние (`true` — теперь в закладках).
    pub fn toggle_bookmark(&self, url: &str, title: &str) -> Result<bool> {
        if self.is_bookmarked(url)? {
            self.remove_bookmark(url)?;
            Ok(false)
        } else {
            self.add_bookmark(url, title)?;
            Ok(recordable(url))
        }
    }

    pub fn bookmarks(&self) -> Result<Vec<Bookmark>> {
        let mut statement = self
            .conn
            .prepare("SELECT url, title, created FROM bookmarks ORDER BY created DESC")?;
        let rows = statement.query_map([], |row| {
            Ok(Bookmark {
                url: row.get(0)?,
                title: row.get(1)?,
                created: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // ---- сессия вкладок ----

    pub fn save_session(&self, session: &SessionState) -> Result<()> {
        if self.dir.as_os_str().is_empty() {
            return Ok(()); // хранилище в памяти
        }
        let text = serde_json::to_string(session).map_err(|e| Error::Format(e.to_string()))?;
        let tmp = self.dir.join("session.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, self.dir.join("session.json"))?;
        Ok(())
    }

    /// Сохранённая сессия; нет файла или он битый — пустая (запуск не должен падать из-за сессии).
    pub fn load_session(&self) -> SessionState {
        std::fs::read_to_string(self.dir.join("session.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<SessionState>(&text).ok())
            .map(|mut session| {
                session.tabs.retain(|tab| recordable(&tab.url));
                session.active = session.active.min(session.tabs.len().saturating_sub(1));
                session
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_accumulate_and_title_updates() {
        let store = Store::in_memory().unwrap();
        store.record_visit("https://example.org/", "").unwrap();
        store
            .record_visit("https://example.org/", "Example")
            .unwrap();
        store.record_visit("https://example.org/", "").unwrap(); // пустой заголовок не затирает
        let entries = store.recent_history(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (entries[0].visits, entries[0].title.as_str()),
            (3, "Example")
        );
    }

    #[test]
    fn internal_urls_are_not_recorded() {
        let store = Store::in_memory().unwrap();
        for url in [
            "about:blank",
            "data:text/html,x",
            "javascript:alert(1)",
            "не url",
        ] {
            store.record_visit(url, "t").unwrap();
            store.add_bookmark(url, "t").unwrap();
        }
        assert!(store.recent_history(10).unwrap().is_empty());
        assert!(store.bookmarks().unwrap().is_empty());
    }

    #[test]
    fn search_is_case_insensitive_and_escapes_wildcards() {
        let store = Store::in_memory().unwrap();
        store
            .record_visit("https://a.example/100%_done", "Прогресс")
            .unwrap();
        store
            .record_visit("https://b.example/other", "Hacker News")
            .unwrap();
        assert_eq!(store.search_history("hacker", 10).unwrap().len(), 1);
        assert_eq!(
            store.search_history("прогресс", 10).unwrap().len(),
            1,
            "кириллица без учёта регистра"
        );
        assert_eq!(store.search_history("ПРОГРЕСС", 10).unwrap().len(), 1);
        assert_eq!(
            store.search_history("%", 10).unwrap().len(),
            1,
            "% — обычный символ, а не маска"
        );
        assert_eq!(store.search_history("_", 10).unwrap().len(), 1);
        assert_eq!(store.search_history("нет такого", 10).unwrap().len(), 0);
    }

    #[test]
    fn history_delete_and_clear() {
        let store = Store::in_memory().unwrap();
        store.record_visit("https://a.example/", "a").unwrap();
        store.record_visit("https://b.example/", "b").unwrap();
        store.delete_history_entry("https://a.example/").unwrap();
        assert_eq!(store.recent_history(10).unwrap().len(), 1);
        store.clear_history().unwrap();
        assert!(store.recent_history(10).unwrap().is_empty());
    }

    #[test]
    fn bookmark_toggle() {
        let store = Store::in_memory().unwrap();
        assert!(
            store
                .toggle_bookmark("https://example.org/", "Example")
                .unwrap()
        );
        assert!(store.is_bookmarked("https://example.org/").unwrap());
        assert!(
            !store
                .toggle_bookmark("https://example.org/", "Example")
                .unwrap()
        );
        assert!(!store.is_bookmarked("https://example.org/").unwrap());
        assert!(
            !store.toggle_bookmark("about:blank", "x").unwrap(),
            "служебные адреса в закладки не попадают"
        );
    }

    #[test]
    fn persists_across_reopen_and_session_is_robust() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            store
                .record_visit("https://example.org/", "Example")
                .unwrap();
            store
                .add_bookmark("https://example.org/", "Example")
                .unwrap();
            store
                .save_session(&SessionState {
                    tabs: vec![
                        TabState {
                            url: "https://a.example/".into(),
                            title: "A".into(),
                        },
                        TabState {
                            url: "about:blank".into(),
                            title: String::new(),
                        },
                        TabState {
                            url: "https://b.example/".into(),
                            title: "B".into(),
                        },
                    ],
                    active: 7,
                })
                .unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.recent_history(10).unwrap().len(), 1);
        assert!(store.is_bookmarked("https://example.org/").unwrap());
        let session = store.load_session();
        assert_eq!(
            session.tabs.len(),
            2,
            "служебные вкладки не восстанавливаются"
        );
        assert_eq!(
            session.active, 1,
            "индекс активной вкладки приводится к допустимому"
        );

        std::fs::write(dir.path().join("session.json"), "{битый json").unwrap();
        assert_eq!(store.load_session(), SessionState::default());
    }
}
