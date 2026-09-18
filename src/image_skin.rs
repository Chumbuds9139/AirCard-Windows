use std::io::{Cursor, Write};
use std::path::Path;

use anyhow::{Context, Result};
use eframe::egui;
use flate2::{Compression, write::ZlibEncoder};
use image::{DynamicImage, GenericImageView, ImageFormat, RgbaImage, imageops::FilterType};

pub const CARD_WIDTH: u32 = 1_536;
pub const CARD_HEIGHT: u32 = 969;

#[derive(Clone)]
pub struct PreparedSkin {
    pub png: Vec<u8>,
    pub pdf: Vec<u8>,
    pub preview: egui::ColorImage,
    pub source_width: u32,
    pub source_height: u32,
}

impl PreparedSkin {
    pub fn from_path(path: &Path) -> Result<Self> {
        let image =
            image::open(path).with_context(|| format!("Could not decode {}", path.display()))?;
        Self::from_image(image)
    }

    pub fn from_image(image: DynamicImage) -> Result<Self> {
        let (source_width, source_height) = image.dimensions();
        let cropped = center_crop_for_card(image);
        let final_image = cropped.resize_exact(CARD_WIDTH, CARD_HEIGHT, FilterType::Lanczos3);
        let rgba = final_image.to_rgba8();
        let pdf = encode_card_pdf(&rgba).context("Could not encode prepared PDF")?;
        let preview = egui::ColorImage::from_rgba_unmultiplied(
            [CARD_WIDTH as usize, CARD_HEIGHT as usize],
            rgba.as_raw(),
        );

        let mut png = Vec::new();
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .context("Could not encode prepared PNG")?;

        Ok(Self {
            png,
            pdf,
            preview,
            source_width,
            source_height,
        })
    }
}

fn center_crop_for_card(image: DynamicImage) -> DynamicImage {
    let (width, height) = image.dimensions();
    let card_ratio = CARD_WIDTH as f64 / CARD_HEIGHT as f64;
    let source_ratio = width as f64 / height as f64;

    if source_ratio > card_ratio {
        let crop_width = (height as f64 * card_ratio).round() as u32;
        let x = (width - crop_width) / 2;
        image.crop_imm(x, 0, crop_width, height)
    } else {
        let crop_height = (width as f64 / card_ratio).round() as u32;
        let y = (height - crop_height) / 2;
        image.crop_imm(0, y, width, crop_height)
    }
}


fn encode_card_pdf(image: &RgbaImage) -> Result<Vec<u8>> {
    let width = image.width();
    let height = image.height();

    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    let mut alpha = Vec::with_capacity((width * height) as usize);
    for pixel in image.pixels() {
        rgb.extend_from_slice(&pixel.0[..3]);
        alpha.push(pixel.0[3]);
    }

    let rgb_stream = deflate(&rgb)?;
    let alpha_stream = deflate(&alpha)?;
    let content = format!("q\n{} 0 0 {} 0 0 cm\n/Im1 Do\nQ\n", width, height);

    let mut pdf = b"%PDF-1.4\n%\xFF\xFF\xFF\xFF\n".to_vec();
    let mut offsets = vec![0usize];

    let mut push_object = |number: usize, body: &[u8]| {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", number).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    };

    push_object(1, b"<< /Type /Catalog /Pages 2 0 R >>");
    push_object(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");

    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /XObject << /Im1 4 0 R >> >> /Contents 6 0 R >>",
        width, height
    );
    push_object(3, page.as_bytes());

    offsets.push(pdf.len());
    pdf.extend_from_slice(b"4 0 obj\n");
    pdf.extend_from_slice(
        format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /SMask 5 0 R /Length {} >>\nstream\n",
            width, height, rgb_stream.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(&rgb_stream);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    offsets.push(pdf.len());
    pdf.extend_from_slice(b"5 0 obj\n");
    pdf.extend_from_slice(
        format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            width, height, alpha_stream.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(&alpha_stream);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    offsets.push(pdf.len());
    pdf.extend_from_slice(b"6 0 obj\n");
    pdf.extend_from_slice(format!("<< /Length {} >>\nstream\n", content.len()).as_bytes());
    pdf.extend_from_slice(content.as_bytes());
    pdf.extend_from_slice(b"endstream\nendobj\n");

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            offsets.len(), xref_offset
        )
        .as_bytes(),
    );

    Ok(pdf)
}

fn deflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).context("Failed to compress PDF image data")?;
    encoder.finish().context("Failed to finish PDF image stream")
}

#[cfg(test)]
mod pdf_tests {
    use super::*;

    #[test]
    fn generated_pdf_has_pdf_header_and_eof() {
        let image = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
        let pdf = encode_card_pdf(&image).expect("PDF should encode");
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.windows(5).any(|w| w == b"%%EOF"));
    }
}
