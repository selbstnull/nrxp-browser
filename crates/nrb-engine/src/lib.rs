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

impl Frame {
    /// Порог «пустой страницы»: если один цвет занимает больше этой доли кадра — страница считается пустой.
    pub const BLANK_RATIO: f64 = 0.995;

    pub fn is_blank(&self) -> bool {
        self.dominant_color_ratio() > Self::BLANK_RATIO
    }

    pub fn write_png(&self, path: &std::path::Path) -> std::io::Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut encoder = png::Encoder::new(file, self.size.width, self.size.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
        writer.write_image_data(&self.rgba).map_err(std::io::Error::other)
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

/// Результат снимка: кадр и наблюдения за загрузкой.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub frame: Frame,
    /// Время от начала навигации до `Complete`.
    pub load_time: Duration,
    /// Сообщения консоли страницы (`console.*`) — нужны корпусу совместимости (I3).
    pub console: Vec<String>,
}

/// Минимальный интерфейс, нужный для headless-снимков. Интерактивная часть (вкладки, ввод) добавляется
/// отдельными трейтами по мере итераций (I2), чтобы не раздувать контракт заранее.
pub trait Snapshotter {
    fn snapshot(&mut self, req: &SnapshotRequest) -> Result<Snapshot, EngineError>;
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
