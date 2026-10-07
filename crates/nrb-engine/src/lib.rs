//! Абстракция движка. Всё, что не является движком (оболочка, сеть, узлы), зависит только от этого крейта,
//! поэтому собирается быстро и не тянет Servo.

use std::time::Duration;
use url::Url;

/// Размер вьюпорта в физических пикселях.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl std::str::FromStr for Size {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (w, h) = s.split_once('x').ok_or("ожидается формат ШИРИНАxВЫСОТА")?;
        let width = w.parse().map_err(|_| "некорректная ширина")?;
        let height = h.parse().map_err(|_| "некорректная высота")?;
        if width == 0 || height == 0 {
            return Err("размер не может быть нулевым".into());
        }
        Ok(Self { width, height })
    }
}

/// Готовый кадр: RGBA8, строки сверху вниз, без отступов.
#[derive(Debug, Clone)]
pub struct Frame {
    pub size: Size,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// Доля пикселей самого частого цвета (0.0..=1.0) — признак «пустой» страницы.
    pub fn dominant_color_ratio(&self) -> f64 {
        use std::collections::HashMap;
        let mut counts: HashMap<[u8; 4], u64> = HashMap::new();
        for px in self.rgba.chunks_exact(4) {
            *counts.entry([px[0], px[1], px[2], px[3]]).or_default() += 1;
        }
        let total = (self.rgba.len() / 4).max(1) as f64;
        counts.values().copied().max().unwrap_or(0) as f64 / total
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("таймаут загрузки ({0:?})")]
    Timeout(Duration),
    #[error("движок упал: {0}")]
    Crashed(String),
    #[error("ошибка движка: {0}")]
    Other(String),
}

/// Параметры одного снимка страницы.
#[derive(Debug, Clone)]
pub struct SnapshotRequest {
    pub url: Url,
    pub size: Size,
    pub timeout: Duration,
    /// Дополнительная пауза после `Complete` (для страниц, дорисовывающихся скриптами).
    pub settle: Duration,
}

/// Минимальный интерфейс, нужный для headless-снимков. Интерактивная часть (вкладки, ввод) добавляется
/// отдельными трейтами по мере итераций (I2), чтобы не раздувать контракт заранее.
pub trait Snapshotter {
    fn snapshot(&mut self, req: &SnapshotRequest) -> Result<Frame, EngineError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_parses() {
        assert_eq!("1280x800".parse::<Size>().unwrap(), Size { width: 1280, height: 800 });
        assert!("0x10".parse::<Size>().is_err());
        assert!("abc".parse::<Size>().is_err());
    }

    #[test]
    fn blank_frame_is_detected() {
        let frame = Frame { size: Size { width: 2, height: 2 }, rgba: vec![255; 16] };
        assert!(frame.dominant_color_ratio() > 0.999);
        let mut rgba = vec![255; 16];
        rgba[0] = 0;
        let frame = Frame { size: Size { width: 2, height: 2 }, rgba };
        assert!(frame.dominant_color_ratio() < 0.8);
    }
}
