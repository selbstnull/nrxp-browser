//! Платформенные различия в одном месте: где лежат данные и загрузки, чем открыть папку. Функции с суффиксом `_for` берут
//! ОС и окружение параметрами — чтобы правила проверялись тестами на любой машине, а не только на той, где собран бинарь.
//!
//! Поддерживаемые ОС: Linux (проверено), Windows и macOS (правила здесь, сборка с движком не проверена), Android (каталоги
//! задаёт оболочка приложения через `NRB_DATA_DIR`/`NRB_DOWNLOADS_DIR`: у процесса нет домашнего каталога в обычном смысле).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Os {
    Linux,
    Windows,
    Mac,
    Android,
}

impl Os {
    pub const fn current() -> Os {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "android") {
            Os::Android
        } else {
            Os::Linux
        }
    }
}

type Env = HashMap<String, String>;

fn current_env() -> Env {
    std::env::vars().collect()
}

fn var<'a>(env: &'a Env, name: &str) -> Option<&'a str> {
    env.get(name).map(String::as_str).filter(|v| !v.is_empty())
}

/// Домашний каталог пользователя.
fn home(os: Os, env: &Env) -> Option<PathBuf> {
    match os {
        Os::Windows => var(env, "USERPROFILE").or_else(|| var(env, "HOME")),
        _ => var(env, "HOME"),
    }
    .map(PathBuf::from)
}

/// Каталог данных браузера: профили, узел, ключи. Явно заданный `NRB_DATA_DIR` важнее всего (Android: путь приложения).
pub fn data_dir_for(os: Os, env: &Env) -> Option<PathBuf> {
    if let Some(dir) = var(env, "NRB_DATA_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Some(dir);
    }
    let base = match os {
        Os::Linux | Os::Android => var(env, "XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home(os, env).map(|h| h.join(".local/share")))?,
        Os::Mac => home(os, env)?.join("Library/Application Support"),
        // Локальные (не роуминговые) данные: профиль с кэшем и ключами узла не должен уезжать на другие машины домена.
        Os::Windows => var(env, "LOCALAPPDATA")
            .or_else(|| var(env, "APPDATA"))
            .map(PathBuf::from)
            .or_else(|| home(os, env).map(|h| h.join("AppData/Local")))?,
    };
    Some(base.join("netrunner-browser"))
}

pub fn data_dir() -> Option<PathBuf> {
    data_dir_for(Os::current(), &current_env())
}

/// Каталог загрузок. Linux: `XDG_DOWNLOAD_DIR` из `user-dirs.dirs` (содержимое файла передаётся параметром), иначе
/// `~/Downloads`; Windows/macOS: `~/Downloads`; `NRB_DOWNLOADS_DIR` важнее всего.
pub fn downloads_dir_for(os: Os, env: &Env, user_dirs: Option<&str>) -> PathBuf {
    if let Some(dir) = var(env, "NRB_DOWNLOADS_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return dir;
    }
    let home = home(os, env).unwrap_or_default();
    if matches!(os, Os::Linux)
        && let Some(text) = user_dirs
    {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") {
                let value = value
                    .trim()
                    .trim_matches('"')
                    .replace("$HOME", &home.to_string_lossy());
                let path = PathBuf::from(value);
                if path.is_absolute() && path != home {
                    return path;
                }
            }
        }
    }
    home.join("Downloads")
}

pub fn downloads_dir() -> PathBuf {
    let env = current_env();
    let os = Os::current();
    let user_dirs = if os == Os::Linux {
        let config = var(&env, "XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home(os, &env).map(|h| h.join(".config")))
            .unwrap_or_default();
        std::fs::read_to_string(config.join("user-dirs.dirs")).ok()
    } else {
        None
    };
    downloads_dir_for(os, &env, user_dirs.as_deref())
}

/// Домашний каталог (для `~/.ssh` и т. п.).
pub fn home_dir() -> Option<PathBuf> {
    home(Os::current(), &current_env())
}

/// Команда, которой ОС открывает путь в файловом менеджере: `(программа, аргументы до пути)`. `None` — не поддерживается.
pub fn opener_for(os: Os) -> Option<(&'static str, &'static [&'static str])> {
    match os {
        Os::Linux => Some(("xdg-open", &[])),
        Os::Mac => Some(("open", &[])),
        // `explorer` возвращает код 1 даже при успехе — результат команды не проверяем.
        Os::Windows => Some(("explorer", &[])),
        Os::Android => None,
    }
}

/// Открывает папку/файл в файловом менеджере ОС (без ожидания).
pub fn open_path(path: &Path) {
    if let Some((program, args)) = opener_for(Os::current()) {
        let _ = std::process::Command::new(program)
            .args(args)
            .arg(path)
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn data_dir_per_platform() {
        assert_eq!(
            data_dir_for(Os::Linux, &env(&[("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.local/share/netrunner-browser"))
        );
        assert_eq!(
            data_dir_for(
                Os::Linux,
                &env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/data")])
            ),
            Some(PathBuf::from("/data/netrunner-browser"))
        );
        // Относительный XDG_DATA_HOME по спецификации игнорируется.
        assert_eq!(
            data_dir_for(Os::Linux, &env(&[("HOME", "/h"), ("XDG_DATA_HOME", "rel")])),
            Some(PathBuf::from("/h/.local/share/netrunner-browser"))
        );
        assert_eq!(
            data_dir_for(Os::Mac, &env(&[("HOME", "/Users/u")])),
            Some(PathBuf::from(
                "/Users/u/Library/Application Support/netrunner-browser"
            ))
        );
        assert_eq!(
            data_dir_for(
                Os::Windows,
                &env(&[
                    ("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local"),
                    ("USERPROFILE", "C:\\Users\\u")
                ])
            ),
            Some(PathBuf::from("C:\\Users\\u\\AppData\\Local").join("netrunner-browser"))
        );
        assert_eq!(
            data_dir_for(Os::Windows, &env(&[("USERPROFILE", "C:\\Users\\u")])),
            Some(
                PathBuf::from("C:\\Users\\u")
                    .join("AppData/Local")
                    .join("netrunner-browser")
            )
        );
        // Android: путь приложения явно.
        assert_eq!(
            data_dir_for(
                Os::Android,
                &env(&[("NRB_DATA_DIR", "/data/user/0/app/files")])
            ),
            Some(PathBuf::from("/data/user/0/app/files"))
        );
        assert_eq!(data_dir_for(Os::Linux, &env(&[])), None);
    }

    #[test]
    fn downloads_dir_per_platform() {
        let e = env(&[("HOME", "/home/u")]);
        assert_eq!(
            downloads_dir_for(Os::Linux, &e, None),
            PathBuf::from("/home/u/Downloads")
        );
        assert_eq!(
            downloads_dir_for(Os::Linux, &e, Some("XDG_DOWNLOAD_DIR=\"$HOME/Загрузки\"\n")),
            PathBuf::from("/home/u/Загрузки")
        );
        // XDG_DOWNLOAD_DIR=$HOME означает «не настроено».
        assert_eq!(
            downloads_dir_for(Os::Linux, &e, Some("XDG_DOWNLOAD_DIR=\"$HOME\"\n")),
            PathBuf::from("/home/u/Downloads")
        );
        assert_eq!(
            downloads_dir_for(Os::Mac, &env(&[("HOME", "/Users/u")]), None),
            PathBuf::from("/Users/u/Downloads")
        );
        assert_eq!(
            downloads_dir_for(Os::Windows, &env(&[("USERPROFILE", "C:\\Users\\u")]), None),
            PathBuf::from("C:\\Users\\u").join("Downloads")
        );
        assert_eq!(
            downloads_dir_for(
                Os::Android,
                &env(&[("NRB_DOWNLOADS_DIR", "/sdcard/Download")]),
                None
            ),
            PathBuf::from("/sdcard/Download")
        );
    }

    #[test]
    fn openers() {
        assert_eq!(opener_for(Os::Linux).unwrap().0, "xdg-open");
        assert_eq!(opener_for(Os::Mac).unwrap().0, "open");
        assert_eq!(opener_for(Os::Windows).unwrap().0, "explorer");
        assert!(opener_for(Os::Android).is_none());
    }
}
