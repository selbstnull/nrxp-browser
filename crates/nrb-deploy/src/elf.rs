use crate::{DeployError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryArch {
    X86_64,
    Aarch64,
}

impl BinaryArch {
    /// Значение `uname -m` для этой архитектуры.
    pub fn uname(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    pub fn matches_uname(self, uname: &str) -> bool {
        match self {
            Self::X86_64 => matches!(uname.trim(), "x86_64" | "amd64"),
            Self::Aarch64 => matches!(uname.trim(), "aarch64" | "arm64"),
        }
    }
}

/// Архитектура ELF-файла по заголовку: ловит «залили не тот бинарь» до запуска на сервере.
pub fn binary_arch(data: &[u8]) -> Result<BinaryArch> {
    let bad = |why: &str| DeployError::Local(format!("это не Linux-бинарь для сервера: {why}"));
    if data.len() < 20 || &data[..4] != b"\x7fELF" {
        return Err(bad("нет заголовка ELF"));
    }
    if data[4] != 2 {
        return Err(bad("нужен 64-битный ELF"));
    }
    let little = data[5] == 1;
    let machine = if little {
        u16::from_le_bytes([data[18], data[19]])
    } else {
        u16::from_be_bytes([data[18], data[19]])
    };
    match machine {
        62 => Ok(BinaryArch::X86_64),
        183 => Ok(BinaryArch::Aarch64),
        other => Err(bad(&format!(
            "архитектура ELF {other} не поддерживается (нужны x86_64/aarch64)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf(class: u8, endian: u8, machine: u16) -> Vec<u8> {
        let mut data = vec![0u8; 64];
        data[..4].copy_from_slice(b"\x7fELF");
        data[4] = class;
        data[5] = endian;
        data[18..20].copy_from_slice(&machine.to_le_bytes());
        data
    }

    #[test]
    fn detects_supported_architectures() {
        assert_eq!(binary_arch(&elf(2, 1, 62)).unwrap(), BinaryArch::X86_64);
        assert_eq!(binary_arch(&elf(2, 1, 183)).unwrap(), BinaryArch::Aarch64);
    }

    #[test]
    fn rejects_foreign_binaries() {
        assert!(binary_arch(b"MZ\x90\x00 windows exe").is_err());
        assert!(binary_arch(&elf(1, 1, 3)).is_err(), "32-бит");
        assert!(binary_arch(&elf(2, 1, 40)).is_err(), "ARM32");
        assert!(binary_arch(&[]).is_err());
    }

    #[test]
    fn uname_matching() {
        assert!(BinaryArch::X86_64.matches_uname("x86_64\n"));
        assert!(BinaryArch::Aarch64.matches_uname("arm64"));
        assert!(!BinaryArch::X86_64.matches_uname("aarch64"));
    }
}
