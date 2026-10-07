use async_trait::async_trait;

use crate::Result;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub code: u32,
    pub stdout: String,
    pub stderr: String,
}

impl ExecOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

/// Удалённая машина: выполнить команду, залить файл. Всё остальное строится поверх этого.
#[async_trait]
pub trait Remote: Send + Sync {
    /// Выполняет команду в `sh -c` на сервере и ждёт завершения. Ненулевой код — не ошибка вызова, а часть результата.
    async fn exec(&self, command: &str) -> Result<ExecOutput>;
    /// Записывает `data` в `remote_path` (владелец — пользователь SSH) и выставляет `mode`.
    async fn upload(&self, remote_path: &str, data: &[u8], mode: u32) -> Result<()>;
}

/// Экранирование для POSIX-шелла: значение всегда в одинарных кавычках. Всё, что пришло от пользователя
/// (имена устройств, адреса), обязано проходить через эту функцию — команды исполняются от root.
pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_neutralizes_shell_metacharacters() {
        assert_eq!(sh_quote("телефон"), "'телефон'");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("x'; rm -rf / #"), "'x'\\''; rm -rf / #'");
        assert_eq!(sh_quote("$(reboot)"), "'$(reboot)'");
        assert_eq!(sh_quote(""), "''");
    }
}
