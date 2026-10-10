#![cfg_attr(not(feature = "servo"), allow(dead_code))]
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
    Some(crate::platform::data_dir()?.join("profile"))
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

impl ProfileArgs {
    /// Каталог списков фильтров блокировщика: `<профиль>/filters`. В приватном сеансе списки читаются из профиля по
    /// умолчанию (они не личные данные), но ничего туда не пишется.
    /// Каталог пользовательских скриптов и стилей (`*.user.js`, `*.user.css`): `<профиль>/userscripts`.
    pub fn userscripts_dir(&self) -> Option<PathBuf> {
        self.profile
            .clone()
            .or_else(default_dir)
            .map(|d| d.join("userscripts"))
    }

    pub fn filters_dir(&self) -> Option<PathBuf> {
        self.profile
            .clone()
            .or_else(default_dir)
            .map(|d| d.join("filters"))
    }
}
