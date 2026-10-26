//! Bounded static-image validation and metadata-free normalization.

use std::io::Cursor;

use image::{
    DynamicImage, GenericImageView, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
    codecs::png::PngEncoder, imageops::FilterType,
};

use crate::AssetStoreError;

/// Maximum accepted encoded image length: 20 MiB.
pub const MAX_ENCODED_BYTES: usize = 20 * 1024 * 1024;
/// Maximum accepted decoded image pixels: 40 megapixels.
pub const MAX_DECODED_PIXELS: u64 = 40_000_000;
/// Maximum normalized inference-derivative pixels: 4 megapixels.
pub const MAX_INFERENCE_PIXELS: u64 = 4_000_000;
const MAX_AXIS: u32 = 16_384;
const MAX_INFERENCE_AXIS: u32 = 2_048;
const MAX_WORKING_ALLOCATION: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SupportedFormat {
    Jpeg,
    Png,
    WebP,
}

impl SupportedFormat {
    pub(crate) const fn image_format(self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
            Self::WebP => ImageFormat::WebP,
        }
    }

    pub(crate) const fn media_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
        }
    }

    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
        }
    }
}

pub(crate) struct NormalizedImage {
    pub(crate) source_format: SupportedFormat,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) derivative_width: u32,
    pub(crate) derivative_height: u32,
    pub(crate) derivative_png: Vec<u8>,
}

pub(crate) fn normalize(bytes: &[u8]) -> Result<NormalizedImage, AssetStoreError> {
    if bytes.is_empty() {
        return Err(AssetStoreError::InvalidMedia);
    }
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(AssetStoreError::LimitExceeded);
    }
    let source_format = inspect_container(bytes)?;
    let mut codec = ImageReader::with_format(Cursor::new(bytes), source_format.image_format())
        .into_decoder()
        .map_err(classify_codec_error)?;
    let (encoded_width, encoded_height) = codec.dimensions();
    validate_dimensions(encoded_width, encoded_height)?;
    let decoded_bytes = codec.total_bytes();
    let full_rgba_bytes = pixel_count(encoded_width, encoded_height)?.saturating_mul(4);
    if decoded_bytes.saturating_add(full_rgba_bytes) > MAX_WORKING_ALLOCATION {
        return Err(AssetStoreError::LimitExceeded);
    }
    let orientation = codec.orientation().map_err(classify_codec_error)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_AXIS);
    limits.max_image_height = Some(MAX_AXIS);
    limits.max_alloc = Some(MAX_WORKING_ALLOCATION);
    codec.set_limits(limits).map_err(classify_codec_error)?;
    let mut decoded = DynamicImage::from_decoder(codec).map_err(classify_codec_error)?;
    decoded.apply_orientation(orientation);
    let (width, height) = decoded.dimensions();
    validate_dimensions(width, height)?;

    let (derivative_width, derivative_height) = derivative_dimensions(width, height)?;
    let derivative = if (width, height) == (derivative_width, derivative_height) {
        decoded
    } else {
        decoded.resize_exact(derivative_width, derivative_height, FilterType::Triangle)
    };
    let rgba = derivative.into_rgba8();
    let mut derivative_png = Vec::new();
    PngEncoder::new(&mut derivative_png)
        .write_image(
            rgba.as_raw(),
            derivative_width,
            derivative_height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(classify_codec_error)?;

    Ok(NormalizedImage {
        source_format,
        width,
        height,
        derivative_width,
        derivative_height,
        derivative_png,
    })
}

fn inspect_container(bytes: &[u8]) -> Result<SupportedFormat, AssetStoreError> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        if !bytes.ends_with(&[0xFF, 0xD9]) {
            return Err(AssetStoreError::InvalidMedia);
        }
        return Ok(SupportedFormat::Jpeg);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        inspect_png(bytes)?;
        return Ok(SupportedFormat::Png);
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        inspect_webp(bytes)?;
        return Ok(SupportedFormat::WebP);
    }
    Err(AssetStoreError::UnsupportedMedia)
}

fn inspect_png(bytes: &[u8]) -> Result<(), AssetStoreError> {
    let mut offset = 8_usize;
    let mut saw_iend = false;
    while offset < bytes.len() {
        let header_end = offset.checked_add(8).ok_or(AssetStoreError::InvalidMedia)?;
        let header = bytes
            .get(offset..header_end)
            .ok_or(AssetStoreError::InvalidMedia)?;
        let length = usize::try_from(u32::from_be_bytes(
            header[0..4]
                .try_into()
                .map_err(|_| AssetStoreError::InvalidMedia)?,
        ))
        .map_err(|_| AssetStoreError::InvalidMedia)?;
        let kind = &header[4..8];
        let chunk_end = header_end
            .checked_add(length)
            .and_then(|end| end.checked_add(4))
            .ok_or(AssetStoreError::InvalidMedia)?;
        if chunk_end > bytes.len() || kind == b"acTL" {
            return Err(AssetStoreError::InvalidMedia);
        }
        offset = chunk_end;
        if kind == b"IEND" {
            if length != 0 || offset != bytes.len() {
                return Err(AssetStoreError::InvalidMedia);
            }
            saw_iend = true;
            break;
        }
    }
    if !saw_iend {
        return Err(AssetStoreError::InvalidMedia);
    }
    Ok(())
}

fn inspect_webp(bytes: &[u8]) -> Result<(), AssetStoreError> {
    let declared = bytes
        .get(4..8)
        .ok_or(AssetStoreError::InvalidMedia)?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| AssetStoreError::InvalidMedia)?;
    if u64::from(declared).saturating_add(8)
        != u64::try_from(bytes.len()).map_err(|_| AssetStoreError::InvalidMedia)?
    {
        return Err(AssetStoreError::InvalidMedia);
    }
    let mut offset = 12_usize;
    while offset < bytes.len() {
        let header_end = offset.checked_add(8).ok_or(AssetStoreError::InvalidMedia)?;
        let header = bytes
            .get(offset..header_end)
            .ok_or(AssetStoreError::InvalidMedia)?;
        let kind = &header[0..4];
        let length = usize::try_from(u32::from_le_bytes(
            header[4..8]
                .try_into()
                .map_err(|_| AssetStoreError::InvalidMedia)?,
        ))
        .map_err(|_| AssetStoreError::InvalidMedia)?;
        if kind == b"ANIM" || kind == b"ANMF" {
            return Err(AssetStoreError::InvalidMedia);
        }
        if kind == b"VP8X" {
            let flags = *bytes.get(header_end).ok_or(AssetStoreError::InvalidMedia)?;
            if flags & 0x02 != 0 {
                return Err(AssetStoreError::InvalidMedia);
            }
        }
        let padded = length
            .checked_add(length & 1)
            .ok_or(AssetStoreError::InvalidMedia)?;
        offset = header_end
            .checked_add(padded)
            .ok_or(AssetStoreError::InvalidMedia)?;
        if offset > bytes.len() {
            return Err(AssetStoreError::InvalidMedia);
        }
    }
    if offset != bytes.len() {
        return Err(AssetStoreError::InvalidMedia);
    }
    Ok(())
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), AssetStoreError> {
    if width == 0 || height == 0 || width > MAX_AXIS || height > MAX_AXIS {
        return Err(AssetStoreError::LimitExceeded);
    }
    let _ = pixel_count(width, height)?;
    Ok(())
}

fn pixel_count(width: u32, height: u32) -> Result<u64, AssetStoreError> {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels > MAX_DECODED_PIXELS {
        return Err(AssetStoreError::LimitExceeded);
    }
    Ok(pixels)
}

fn derivative_dimensions(width: u32, height: u32) -> Result<(u32, u32), AssetStoreError> {
    let pixels = pixel_count(width, height)?;
    if width <= MAX_INFERENCE_AXIS && height <= MAX_INFERENCE_AXIS && pixels <= MAX_INFERENCE_PIXELS
    {
        return Ok((width, height));
    }
    let mut low = 1_u32;
    let mut high = width.max(height).min(MAX_INFERENCE_AXIS);
    let mut best = (1_u32, 1_u32);
    while low <= high {
        let candidate = low + (high - low) / 2;
        let dimensions = dimensions_for_long_edge(width, height, candidate)?;
        let candidate_pixels = u64::from(dimensions.0) * u64::from(dimensions.1);
        if candidate_pixels <= MAX_INFERENCE_PIXELS {
            best = dimensions;
            low = candidate.saturating_add(1);
        } else {
            high = candidate.saturating_sub(1);
        }
    }
    Ok(best)
}

fn dimensions_for_long_edge(
    width: u32,
    height: u32,
    long_edge: u32,
) -> Result<(u32, u32), AssetStoreError> {
    if width >= height {
        let scaled = u64::from(height) * u64::from(long_edge) / u64::from(width);
        let scaled = u32::try_from(scaled.max(1)).map_err(|_| AssetStoreError::LimitExceeded)?;
        Ok((long_edge, scaled))
    } else {
        let scaled = u64::from(width) * u64::from(long_edge) / u64::from(height);
        let scaled = u32::try_from(scaled.max(1)).map_err(|_| AssetStoreError::LimitExceeded)?;
        Ok((scaled, long_edge))
    }
}

fn classify_codec_error(error: image::ImageError) -> AssetStoreError {
    if matches!(error, image::ImageError::Limits(_)) {
        AssetStoreError::LimitExceeded
    } else {
        AssetStoreError::Codec(error)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

    use super::*;

    fn png(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
        let image = RgbaImage::from_pixel(width, height, Rgba([20, 40, 60, 255]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image).write_to(&mut bytes, ImageFormat::Png)?;
        Ok(bytes.into_inner())
    }

    #[test]
    fn normalizes_static_png_to_metadata_free_bounded_png() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = png(3_000, 1_500)?;
        let normalized = normalize(&source)?;
        assert_eq!(normalized.source_format, SupportedFormat::Png);
        assert_eq!((normalized.width, normalized.height), (3_000, 1_500));
        assert!(normalized.derivative_width <= MAX_INFERENCE_AXIS);
        assert!(normalized.derivative_height <= MAX_INFERENCE_AXIS);
        assert!(
            u64::from(normalized.derivative_width) * u64::from(normalized.derivative_height)
                <= MAX_INFERENCE_PIXELS
        );
        assert!(normalized.derivative_png.starts_with(b"\x89PNG\r\n\x1a\n"));
        Ok(())
    }

    #[test]
    fn rejects_unsupported_truncated_animated_and_polyglot_inputs()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(matches!(
            normalize(b"<svg/>"),
            Err(AssetStoreError::UnsupportedMedia)
        ));
        assert!(normalize(b"\x89PNG\r\n\x1a\n").is_err());

        let mut animated = png(2, 2)?;
        let iend = animated.len() - 12;
        let mut actl = Vec::new();
        actl.extend_from_slice(&8_u32.to_be_bytes());
        actl.extend_from_slice(b"acTL");
        actl.extend_from_slice(&[0; 12]);
        animated.splice(iend..iend, actl);
        assert!(matches!(
            normalize(&animated),
            Err(AssetStoreError::InvalidMedia)
        ));

        let mut polyglot = png(2, 2)?;
        polyglot.extend_from_slice(b"unexpected trailing bytes");
        assert!(matches!(
            normalize(&polyglot),
            Err(AssetStoreError::InvalidMedia)
        ));
        Ok(())
    }

    #[test]
    fn rejects_encoded_size_and_dimension_limits() {
        let oversized = vec![0_u8; MAX_ENCODED_BYTES + 1];
        assert!(matches!(
            normalize(&oversized),
            Err(AssetStoreError::LimitExceeded)
        ));
        assert!(matches!(
            validate_dimensions(MAX_AXIS + 1, 1),
            Err(AssetStoreError::LimitExceeded)
        ));
        assert!(matches!(
            validate_dimensions(10_000, 10_000),
            Err(AssetStoreError::LimitExceeded)
        ));
    }

    #[test]
    fn inference_derivative_does_not_retain_source_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255]));
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image).write_to(&mut encoded, ImageFormat::Jpeg)?;
        let mut source = encoded.into_inner();
        let payload = b"Exif\0\0GPS_SECRET";
        let length = u16::try_from(payload.len() + 2)?;
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend_from_slice(&length.to_be_bytes());
        app1.extend_from_slice(payload);
        source.splice(2..2, app1);
        assert!(source.windows(10).any(|window| window == b"GPS_SECRET"));

        let normalized = normalize(&source)?;
        assert!(
            !normalized
                .derivative_png
                .windows(10)
                .any(|window| window == b"GPS_SECRET")
        );
        Ok(())
    }
}
