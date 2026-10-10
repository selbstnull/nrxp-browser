//! Растровая печать (N6a): Servo 0.7 не умеет страничную вёрстку, поэтому «печать» — это снимок страницы целиком, нарезанный на листы A4
//! и собранный в PDF картинками (JPEG, `DCTDecode`). Текст в таком PDF не выделяется и не ищется — это честное ограничение
//! способа; настоящий PDF с текстом возможен только после страничной вёрстки в движке (N6 форка).

use image::{ImageEncoder, RgbaImage, codecs::jpeg::JpegEncoder};

/// Размер листа A4 в пунктах PDF (1/72 дюйма).
pub const A4_WIDTH_PT: f32 = 595.0;
pub const A4_HEIGHT_PT: f32 = 842.0;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("пустое изображение")]
    Empty,
    #[error("кодирование JPEG: {0}")]
    Encode(String),
}

struct Page {
    jpeg: Vec<u8>,
    width: u32,
    height: u32,
}

/// Сколько листов получится из изображения этой высоты при данной ширине.
pub fn page_count(width: u32, height: u32) -> u32 {
    let page_height = page_height_px(width);
    height.div_ceil(page_height).max(1)
}

fn page_height_px(width: u32) -> u32 {
    ((width as f32) * A4_HEIGHT_PT / A4_WIDTH_PT)
        .round()
        .max(1.0) as u32
}

/// Склеивает снимки видимой области (позиция по вертикали в пикселях снимка + изображение) в один длинный снимок высотой
/// `total_height`; непокрытое остаётся белым, более поздние снимки перекрывают более ранние.
pub fn stitch(tiles: &[(u32, RgbaImage)], width: u32, total_height: u32) -> RgbaImage {
    let mut canvas = RgbaImage::from_pixel(
        width,
        total_height.max(1),
        image::Rgba([255, 255, 255, 255]),
    );
    for (y, tile) in tiles {
        image::imageops::replace(&mut canvas, tile, 0, i64::from(*y));
    }
    canvas
}

/// Нарезает `full` на листы A4 (последний добивается белым), кодирует JPEG (качество `quality`, 1–100) и собирает PDF.
pub fn pdf_from_image(full: &RgbaImage, title: &str, quality: u8) -> Result<Vec<u8>, Error> {
    let (width, height) = full.dimensions();
    if width == 0 || height == 0 {
        return Err(Error::Empty);
    }
    let page_height = page_height_px(width);
    let mut pages = Vec::new();
    let mut y = 0;
    while y < height {
        let rows = (height - y).min(page_height);
        // Лист: белый фон + кусок снимка (прозрачность сводится к белому).
        let mut rgb = vec![255u8; (width * page_height * 3) as usize];
        for row in 0..rows {
            for x in 0..width {
                let pixel = full.get_pixel(x, y + row).0;
                let alpha = u32::from(pixel[3]);
                let at = ((row * width + x) * 3) as usize;
                for c in 0..3 {
                    let blended = (u32::from(pixel[c]) * alpha + 255 * (255 - alpha)) / 255;
                    rgb[at + c] = blended as u8;
                }
            }
        }
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, quality.clamp(1, 100))
            .write_image(&rgb, width, page_height, image::ExtendedColorType::Rgb8)
            .map_err(|e| Error::Encode(e.to_string()))?;
        pages.push(Page {
            jpeg,
            width,
            height: page_height,
        });
        y += page_height;
    }
    Ok(write_pdf(&pages, title))
}

/// Заголовок документа как PDF-строка: ASCII — в скобках, остальное — UTF-16BE с BOM в hex.
fn pdf_text_string(text: &str) -> String {
    if text.is_ascii() && !text.contains(['(', ')', '\\']) {
        return format!("({text})");
    }
    let mut out = String::from("<FEFF");
    for unit in text.encode_utf16() {
        out.push_str(&format!("{unit:04X}"));
    }
    out.push('>');
    out
}

fn write_pdf(pages: &[Page], title: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new(); // смещение объекта n (нумерация с 1) — offsets[n-1]
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    // Нумерация: 1 — Catalog, 2 — Pages, 3 — Info; дальше для каждого листа: страница, содержимое, картинка.
    let first = 4;
    let kids: String = (0..pages.len())
        .map(|i| format!("{} 0 R", first + i * 3))
        .collect::<Vec<_>>()
        .join(" ");
    let mut push = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        let number = offsets.len();
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    push(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    push(
        &mut out,
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).as_bytes(),
    );
    push(
        &mut out,
        format!(
            "<< /Title {} /Producer (nrb raster print) >>",
            pdf_text_string(title)
        )
        .as_bytes(),
    );
    for (i, page) in pages.iter().enumerate() {
        let number = first + i * 3;
        push(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {A4_WIDTH_PT} {A4_HEIGHT_PT}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                number + 2,
                number + 1
            )
            .as_bytes(),
        );
        let content = format!("q {A4_WIDTH_PT} 0 0 {A4_HEIGHT_PT} 0 0 cm /Im0 Do Q");
        push(
            &mut out,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .as_bytes(),
        );
        let mut image = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            page.width,
            page.height,
            page.jpeg.len()
        )
        .into_bytes();
        image.extend_from_slice(&page.jpeg);
        image.extend_from_slice(b"\nendstream");
        push(&mut out, &image);
    }

    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        })
    }

    #[test]
    fn pages_follow_the_a4_ratio() {
        assert_eq!(page_height_px(595), 842);
        assert_eq!(page_count(595, 842), 1);
        assert_eq!(page_count(595, 843), 2);
        assert_eq!(page_count(1000, 10), 1);
        assert_eq!(page_count(595, 842 * 5), 5);
    }

    #[test]
    fn pdf_structure_is_consistent() {
        let pdf = pdf_from_image(&gradient(600, 2000), "Тест", 80).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let text = String::from_utf8_lossy(&pdf);
        let pages = page_count(600, 2000);
        assert!(text.contains(&format!("/Count {pages}")));
        // xref указывает ровно на начала объектов
        let startxref: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[startxref..].starts_with(b"xref"));
        // `text` — результат lossy-преобразования бинарного файла (JPEG), его индексы не совпадают с байтовыми смещениями.
        let table_text = String::from_utf8_lossy(&pdf[startxref..]).into_owned();
        let table = table_text.as_str();
        let entries: Vec<usize> = table
            .lines()
            .skip(3)
            .take_while(|l| l.ends_with(" n "))
            .map(|l| l[..10].parse().unwrap())
            .collect();
        assert_eq!(entries.len() as u32, 3 + pages * 3);
        for (i, offset) in entries.iter().enumerate() {
            let expected = format!("{} 0 obj", i + 1);
            assert!(
                pdf[*offset..].starts_with(expected.as_bytes()),
                "объект {} не по смещению",
                i + 1
            );
        }
        // заголовок не ASCII — UTF-16BE с BOM
        assert!(text.contains("<FEFF"));
    }

    #[test]
    fn tiles_are_stitched_with_later_ones_on_top() {
        let red = RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]));
        let blue = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 255, 255]));
        let out = stitch(&[(0, red), (5, blue)], 10, 20);
        assert_eq!(out.dimensions(), (10, 20));
        assert_eq!(out.get_pixel(0, 2).0, [255, 0, 0, 255]);
        assert_eq!(
            out.get_pixel(0, 7).0,
            [0, 0, 255, 255],
            "перекрытие — верхний снимок"
        );
        assert_eq!(
            out.get_pixel(0, 18).0,
            [255, 255, 255, 255],
            "непокрытое — белое"
        );
    }

    #[test]
    fn empty_image_is_an_error() {
        assert!(matches!(
            pdf_from_image(&RgbaImage::new(0, 0), "x", 80),
            Err(Error::Empty)
        ));
    }

    #[test]
    fn real_pdf_tools_accept_the_file_when_available() {
        let Ok(poppler) = std::process::Command::new("pdfinfo").arg("-v").output() else {
            return; // нет poppler — структурная проверка выше остаётся
        };
        let _ = poppler;
        let dir = std::env::temp_dir().join(format!("nrb-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.pdf");
        std::fs::write(
            &path,
            pdf_from_image(&gradient(600, 2000), "Проверка", 80).unwrap(),
        )
        .unwrap();
        let info = std::process::Command::new("pdfinfo")
            .arg(&path)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&info.stdout).into_owned();
        assert!(
            info.status.success(),
            "pdfinfo: {}",
            String::from_utf8_lossy(&info.stderr)
        );
        assert!(
            text.contains(&format!("Pages:           {}", page_count(600, 2000))),
            "{text}"
        );
        assert!(text.contains("595 x 842 pts"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
