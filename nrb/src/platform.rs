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

/// Разделитель путей целевой ОС (не той, где собран бинарь: правила проверяются тестами на любой машине).
fn sep(os: Os) -> char {
    if os == Os::Windows { '\\' } else { '/' }
}

/// Склейка пути по правилам целевой ОС: `a` + `b` с её разделителем (разделители в `b` приводятся к нему же).
fn join_for(os: Os, base: &str, rest: &str) -> String {
    let separator = sep(os);
    let rest: String = rest
        .chars()
        .map(|c| if c == '/' || c == '\\' { separator } else { c })
        .collect();
    format!(
        "{}{}{}",
        base.trim_end_matches(['/', '\\']),
        separator,
        rest
    )
}

/// Абсолютный ли путь по правилам целевой ОС (`/…` на Unix; `C:\…`, `C:/…` и `\\сервер\…` на Windows).
fn is_absolute_for(os: Os, path: &str) -> bool {
    match os {
        Os::Windows => {
            let b = path.as_bytes();
            (b.len() >= 3
                && b[1] == b':'
                && b[0].is_ascii_alphabetic()
                && (b[2] == b'\\' || b[2] == b'/'))
                || path.starts_with("\\\\")
        }
        _ => path.starts_with('/'),
    }
}

/// Домашний каталог пользователя.
fn home(os: Os, env: &Env) -> Option<String> {
    match os {
        Os::Windows => var(env, "USERPROFILE").or_else(|| var(env, "HOME")),
        _ => var(env, "HOME"),
    }
    .map(str::to_owned)
}

fn absolute_var<'a>(os: Os, env: &'a Env, name: &str) -> Option<&'a str> {
    var(env, name).filter(|v| is_absolute_for(os, v))
}

/// Каталог данных браузера: профили, узел, ключи. Явно заданный `NRB_DATA_DIR` важнее всего (Android: путь приложения).
pub fn data_dir_for(os: Os, env: &Env) -> Option<PathBuf> {
    if let Some(dir) = absolute_var(os, env, "NRB_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = match os {
        Os::Linux | Os::Android => match absolute_var(os, env, "XDG_DATA_HOME") {
            Some(dir) => dir.to_owned(),
            None => join_for(os, &home(os, env)?, ".local/share"),
        },
        Os::Mac => join_for(os, &home(os, env)?, "Library/Application Support"),
        // Локальные (не роуминговые) данные: профиль с кэшем и ключами узла не должен уезжать на другие машины домена.
        Os::Windows => match var(env, "LOCALAPPDATA").or_else(|| var(env, "APPDATA")) {
            Some(dir) => dir.to_owned(),
            None => join_for(os, &home(os, env)?, "AppData/Local"),
        },
    };
    Some(PathBuf::from(join_for(os, &base, "netrunner-browser")))
}

pub fn data_dir() -> Option<PathBuf> {
    data_dir_for(Os::current(), &current_env())
}

/// Каталог загрузок. Linux: `XDG_DOWNLOAD_DIR` из `user-dirs.dirs` (содержимое файла передаётся параметром), иначе
/// `~/Downloads`; Windows/macOS: `~/Downloads`; `NRB_DOWNLOADS_DIR` важнее всего.
pub fn downloads_dir_for(os: Os, env: &Env, user_dirs: Option<&str>) -> PathBuf {
    if let Some(dir) = absolute_var(os, env, "NRB_DOWNLOADS_DIR") {
        return PathBuf::from(dir);
    }
    let home = home(os, env).unwrap_or_default();
    if matches!(os, Os::Linux)
        && let Some(text) = user_dirs
    {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") {
                let value = value.trim().trim_matches('"').replace("$HOME", &home);
                if is_absolute_for(os, &value) && value != home {
                    return PathBuf::from(value);
                }
            }
        }
    }
    PathBuf::from(join_for(os, &home, "Downloads"))
}

pub fn downloads_dir() -> PathBuf {
    let env = current_env();
    let os = Os::current();
    let user_dirs = if os == Os::Linux {
        let config = var(&env, "XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home(os, &env).map(|h| PathBuf::from(join_for(os, &h, ".config"))))
            .unwrap_or_default();
        std::fs::read_to_string(config.join("user-dirs.dirs")).ok()
    } else {
        None
    };
    downloads_dir_for(os, &env, user_dirs.as_deref())
}

/// Домашний каталог (для `~/.ssh` и т. п.).
pub fn home_dir() -> Option<PathBuf> {
    home(Os::current(), &current_env()).map(PathBuf::from)
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
            Some(PathBuf::from(
                "C:\\Users\\u\\AppData\\Local\\netrunner-browser"
            ))
        );
        assert_eq!(
            data_dir_for(Os::Windows, &env(&[("USERPROFILE", "C:\\Users\\u")])),
            Some(PathBuf::from(
                "C:\\Users\\u\\AppData\\Local\\netrunner-browser"
            ))
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
            PathBuf::from("C:\\Users\\u\\Downloads")
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
    fn absoluteness_follows_the_target_os() {
        assert!(is_absolute_for(Os::Linux, "/data"));
        assert!(!is_absolute_for(Os::Linux, "rel/path"));
        assert!(!is_absolute_for(Os::Linux, "C:\\x"));
        assert!(is_absolute_for(Os::Windows, "C:\\Users"));
        assert!(is_absolute_for(Os::Windows, "d:/data"));
        assert!(is_absolute_for(Os::Windows, "\\\\server\\share"));
        assert!(!is_absolute_for(Os::Windows, "/data"));
        assert_eq!(join_for(Os::Windows, "C:\\a\\", "b/c"), "C:\\a\\b\\c");
        assert_eq!(join_for(Os::Linux, "/a/", "b\\c"), "/a/b/c");
    }

    #[test]
    fn openers() {
        assert_eq!(opener_for(Os::Linux).unwrap().0, "xdg-open");
        assert_eq!(opener_for(Os::Mac).unwrap().0, "open");
        assert_eq!(opener_for(Os::Windows).unwrap().0, "explorer");
        assert!(opener_for(Os::Android).is_none());
    }
}
