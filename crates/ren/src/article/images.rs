// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Images for the article view, prepared before Blitz sees them: refused
//! above a pixel limit read from the header, and scaled down to the size
//! they are shown at.
//!
//! Blitz decodes every image at full resolution with the `image` crate's
//! default limits (512 MiB per allocation, no limit on the dimensions) and
//! keeps it as RGBA: a 4000×3000 photo costs 46 MiB, and a small file can
//! decode to gigabytes. Blitz only takes bytes, so a scaled-down image is
//! handed to it as a PNG.

use std::io::Cursor;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageFormat, ImageReader};

/// Images with more pixels than this are not shown at all: decoding one
/// would cost too much memory, if only for a moment (25 million pixels are
/// 100 MiB as RGBA, 75 MiB for a progressive JPEG).
pub const MAX_SOURCE_PIXELS: u64 = 25_000_000;

/// The size images are scaled down to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Widest image kept, in pixels: the widest an image is shown at.
    pub max_width: u32,
    /// Most pixels of an image kept (very tall images).
    pub max_pixels: u64,
}

/// Prepares the bytes of an image for Blitz. `Ok(None)` keeps them as they
/// are: an image within the limits, or something that isn't a raster
/// image the `image` crate knows (e.g. SVG, which Blitz draws as vectors).
/// `Ok(Some(png))` replaces them with a scaled-down image. An error refuses
/// the image.
pub fn prepare(bytes: &[u8], limits: Limits) -> Result<Option<Vec<u8>>, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    let Some(format) = reader.format() else {
        return Ok(None);
    };
    let (width, height) = reader.into_dimensions().map_err(|err| err.to_string())?;
    if u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return Err(format!(
            "{width}×{height} pixels, more than {MAX_SOURCE_PIXELS}"
        ));
    }
    let target = target_size(width, height, limits);
    if target == (width, height) {
        return Ok(None);
    }
    let image = match format {
        // Decoded at a fraction of the size if it is much larger.
        ImageFormat::Jpeg => decode_jpeg_scaled(bytes, target).or_else(|_| decode(bytes))?,
        _ => decode(bytes)?,
    };
    let image = if (image.width(), image.height()) == target {
        image
    } else {
        image.thumbnail_exact(target.0, target.1)
    };
    encode_png(image).map(Some)
}

/// The size of an image of `width`×`height` within `limits`, keeping its
/// aspect ratio. Images are never scaled up.
pub fn target_size(width: u32, height: u32, limits: Limits) -> (u32, u32) {
    let (w, h) = (f64::from(width), f64::from(height));
    let by_width = f64::from(limits.max_width.max(1)) / w;
    let by_pixels = (limits.max_pixels.max(1) as f64 / (w * h)).sqrt();
    let factor = by_width.min(by_pixels);
    if factor >= 1.0 {
        return (width, height);
    }
    let scaled = |n: f64| ((n * factor).round() as u32).max(1);
    (scaled(w), scaled(h))
}

fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| err.to_string())?
        .decode()
        .map_err(|err| err.to_string())
}

/// Decodes a JPEG at the smallest of 1/8, 1/4, 1/2 or full size that is at
/// least `target`, which takes a fraction of the memory and time of a full
/// decode. Only grey and RGB images; others go through [`decode`].
fn decode_jpeg_scaled(bytes: &[u8], target: (u32, u32)) -> Result<DynamicImage, String> {
    use jpeg_decoder::{Decoder, PixelFormat};
    let mut decoder = Decoder::new(Cursor::new(bytes));
    let clamp = |n: u32| u16::try_from(n).unwrap_or(u16::MAX);
    decoder
        .scale(clamp(target.0), clamp(target.1))
        .map_err(|err| err.to_string())?;
    let pixels = decoder.decode().map_err(|err| err.to_string())?;
    let info = decoder.info().ok_or("no JPEG header")?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    let image = match info.pixel_format {
        PixelFormat::L8 => image::GrayImage::from_raw(width, height, pixels).map(Into::into),
        PixelFormat::RGB24 => image::RgbImage::from_raw(width, height, pixels).map(Into::into),
        PixelFormat::L16 | PixelFormat::CMYK32 => return Err("not grey or RGB".to_owned()),
    };
    image.ok_or_else(|| "JPEG decoded to the wrong size".to_owned())
}

/// A quickly encoded PNG: it is decoded again right away by Blitz.
fn encode_png(image: DynamicImage) -> Result<Vec<u8>, String> {
    let image = match image {
        DynamicImage::ImageLuma8(_)
        | DynamicImage::ImageLumaA8(_)
        | DynamicImage::ImageRgb8(_)
        | DynamicImage::ImageRgba8(_) => image,
        image if image.color().has_alpha() => image.into_rgba8().into(),
        image => image.into_rgb8().into(),
    };
    let mut png = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive);
    image
        .write_with_encoder(encoder)
        .map_err(|err| err.to_string())?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};

    const LIMITS: Limits = Limits {
        max_width: 100,
        max_pixels: 100 * 400,
    };

    fn png(image: &DynamicImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, y| Rgb([x as u8, y as u8, 128]));
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .write_image(&image, width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    fn dimensions(bytes: &[u8]) -> (u32, u32) {
        let image = image::load_from_memory(bytes).unwrap();
        (image.width(), image.height())
    }

    #[test]
    fn target_sizes() {
        assert_eq!(target_size(50, 50, LIMITS), (50, 50));
        assert_eq!(target_size(100, 400, LIMITS), (100, 400));
        assert_eq!(target_size(400, 100, LIMITS), (100, 25));
        assert_eq!(target_size(1000, 1, LIMITS), (100, 1));
        // Too many pixels even at the full width.
        assert_eq!(target_size(100, 1600, LIMITS), (50, 800));
        // The width doesn't go below a pixel.
        assert_eq!(target_size(1, 100_000, LIMITS), (1, 63_246));
    }

    #[test]
    fn small_images_are_kept() {
        let image = DynamicImage::from(RgbaImage::new(100, 40));
        assert_eq!(prepare(&png(&image), LIMITS), Ok(None));
        assert_eq!(prepare(&jpeg(60, 60), LIMITS), Ok(None));
    }

    #[test]
    fn large_images_are_scaled_down() {
        let image = DynamicImage::from(RgbaImage::from_pixel(400, 200, Rgba([255, 0, 0, 128])));
        let scaled = prepare(&png(&image), LIMITS).unwrap().unwrap();
        assert_eq!(dimensions(&scaled), (100, 50));
        let decoded = image::load_from_memory(&scaled).unwrap().into_rgba8();
        assert_eq!(decoded.get_pixel(50, 25), &Rgba([255, 0, 0, 128]));

        // At 1/4 of the size by the JPEG decoder, then to the exact width.
        let scaled = prepare(&jpeg(800, 600), LIMITS).unwrap().unwrap();
        assert_eq!(dimensions(&scaled), (100, 75));
        let scaled = prepare(&jpeg(130, 30), LIMITS).unwrap().unwrap();
        assert_eq!(dimensions(&scaled), (100, 23));
    }

    #[test]
    fn huge_images_are_refused_from_the_header() {
        // A PNG header claiming 6000×6000 pixels, without image data: only
        // the header is read.
        let mut header = png(&DynamicImage::from(RgbImage::new(1, 1)));
        header[16..20].copy_from_slice(&6000u32.to_be_bytes());
        header[20..24].copy_from_slice(&6000u32.to_be_bytes());
        let crc = crc32(&header[12..29]);
        header[29..33].copy_from_slice(&crc.to_be_bytes());
        let err = prepare(&header, LIMITS).unwrap_err();
        assert!(err.contains("6000×6000"), "{err}");
    }

    /// The CRC of PNG chunks.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn other_content_is_left_to_blitz() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4000" height="10"/>"#;
        assert_eq!(prepare(svg, LIMITS), Ok(None));
        assert_eq!(prepare(b"", LIMITS), Ok(None));
    }

    #[test]
    fn broken_images_are_refused() {
        let mut bytes = png(&DynamicImage::from(RgbImage::new(400, 400)));
        bytes.truncate(40);
        assert!(prepare(&bytes, LIMITS).is_err());
    }
}
