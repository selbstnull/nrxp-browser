//! Профиль браузера: каталог с куками, localStorage, IndexedDB, HSTS и дисковым кэшем (хранит Servo).

use std::path::PathBuf;

#[derive(clap::Args, Clone, Debug, Default)]
pub struct ProfileArgs {
    /// Каталог профиля. Для окна по умолчанию `~/.local/share/netrunner-browser/profile`; для `snap` — временный.
    #[arg(long, env = "NRB_PROFILE")]
    pub profile: Option<PathBuf>,
    /// Приватный сеанс: данные на диск не пишутся и пропадают при выходе.
    #[arg(long, conflicts_with = "profile")]
    pub private: bool,
}

fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("netrunner-browser").join("profile"))
}

fn prepare(dir: PathBuf) -> Option<PathBuf> {
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "профиль {}: не удалось создать каталог: {error} — работаем без сохранения",
            dir.display()
        );
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Профиль содержит куки и сессии — закрыт от других пользователей.
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Some(dir)
}

impl ProfileArgs {
    /// Профиль для окна: заданный, иначе по умолчанию; `--private` — без профиля.
    pub fn for_window(&self) -> Option<PathBuf> {
        if self.private {
            return None;
        }
        prepare(self.profile.clone().or_else(default_dir)?)
    }

    /// Профиль для `snap`/`corpus`: только если задан явно (снимки изолированы друг от друга).
    pub fn for_snap(&self) -> Option<PathBuf> {
        if self.private {
            None
        } else {
            self.profile.clone().and_then(prepare)
        }
    }
}
