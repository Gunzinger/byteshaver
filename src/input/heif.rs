//! HEIC/HEIF/HIF/AVIF input decoding via `libheif-rs` (feature `dec-heif`).
//!
//! libheif decodes the whole HEIF container family, including HEVC-coded
//! HEIC images and AV1-coded AVIF stills, and applies the container's
//! geometric transformations (irot/imir, cropping) during decode — the
//! returned pixels are already "upright".
//!
//! Everything in this module is only compiled with the `dec-heif` feature;
//! builds without it fall back to the feature-off stub in
//! [`crate::input::load_source_with_index`].
//!
//! The libheif types are **not** `Send` (image handles must not cross thread
//! boundaries), so a file is fully decoded within one call of
//! [`load_source`]: open context → collect handles → decode → drop.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use image::{DynamicImage, RgbImage, RgbaImage};
use libheif_rs::{ColorSpace, HeifContext, ImageHandle, ItemId, LibHeif, RgbChroma};

use crate::Error;
use crate::input::{ImageContent, SourceImage};
use crate::metadata::ImageMetadata;

/// Content type of XMP metadata items inside a HEIF container.
const XMP_CONTENT_TYPE: &str = "application/rdf+xml";

/// Opens a HEIF container from disk.
// verified against docs.rs/libheif-rs 3.0: HeifContext::read_from_file(&str)
// -> Result<HeifContext<'static>>
fn open_context(path: &Path) -> Result<HeifContext<'static>, Error> {
    let path_str = path.to_str().ok_or_else(|| {
        Error::from_string(format!(
            "HEIC/HEIF path is not valid unicode: {}",
            path.display()
        ))
    })?;
    HeifContext::read_from_file(path_str).map_err(|err| {
        Error::from_string(format!("libheif could not open {}: {err}", path.display()))
    })
}

/// Collects the decodable top-level image handles of a container,
/// primary image first, remaining master images in file order.
///
/// Thumbnails, auxiliary images (e.g. gain maps) and depth maps are
/// intentionally not decoded.
// verified against docs.rs/libheif-rs 3.0: top_level_image_handles(&self) ->
// Vec<ImageHandle> (no Result), primary_image_handle(&self) ->
// Result<ImageHandle>, ImageHandle::item_id(&self) -> ItemId
fn ordered_image_handles(ctx: &HeifContext<'_>) -> Result<Vec<ImageHandle>, Error> {
    let mut handles = ctx.top_level_image_handles();
    if let Ok(primary) = ctx.primary_image_handle() {
        let primary_id = primary.item_id();
        handles.retain(|handle| handle.item_id() != primary_id);
        handles.insert(0, primary);
    }
    if handles.is_empty() {
        return Err(Error::from_string(
            "HEIC/HEIF file does not contain any decodable image".to_string(),
        ));
    }
    Ok(handles)
}

/// Number of decodable master images in the container (quick header-only
/// enumeration, no pixel decode). Used by the multi-image expansion.
pub(crate) fn probe(path: &Path) -> Result<usize, Error> {
    let ctx = open_context(path)?;
    Ok(ctx.top_level_image_handles().len())
}

/// Loads one image of a HEIF container into a [`SourceImage`].
///
/// `image_index` selects the image when `--heif-image-policy all` expanded a
/// multi-image file (`None` decodes the primary image).
pub(crate) fn load_source(path: &Path, image_index: Option<usize>) -> Result<SourceImage, Error> {
    // libheif is C code: contain panics so a broken file cannot kill the
    // whole batch (the context/handles are not unwind-safe, hence the
    // assertion; nothing borrowed escapes the closure).
    let result = panic::catch_unwind(AssertUnwindSafe(|| load_source_inner(path, image_index)));
    match result {
        Ok(inner) => inner,
        Err(payload) => Err(Error::from_string(format!(
            "libheif decoding panicked: {}",
            panic_message(&payload)
        ))),
    }
}

fn load_source_inner(path: &Path, image_index: Option<usize>) -> Result<SourceImage, Error> {
    let ctx = open_context(path)?;
    let handles = ordered_image_handles(&ctx)?;
    let total_images = handles.len();

    let handle = match image_index {
        Some(index) => handles.get(index).ok_or_else(|| {
            Error::from_string(format!(
                "HEIC/HEIF file {} contains {total_images} image(s), but image {index} was requested",
                path.display()
            ))
        })?,
        // ordered_image_handles puts the primary image first and errors on
        // empty containers, so this always yields a handle
        None => handles.first().ok_or_else(|| {
            Error::from_string(format!(
                "HEIC/HEIF file {} does not contain any decodable image",
                path.display()
            ))
        })?,
    };

    let mut metadata = ImageMetadata {
        exif: extract_exif(handle),
        xmp: extract_xmp(handle),
        icc: extract_icc(handle),
        // libheif applies irot/imir during decode; if the Orientation tag
        // cannot be stripped from the EXIF payload below, this is reset to
        // avoid a double rotation downstream
        exif_applied_orientation: true,
    };

    // WS4 contract: the pixels are upright now, so the Orientation tag must
    // not survive into the re-embedded EXIF payload.
    #[cfg(feature = "exif")]
    if let Some(payload) = metadata.exif.take() {
        let (payload, orientation_removed) = strip_orientation_tag(payload);
        metadata.exif_applied_orientation = orientation_removed;
        metadata.exif = payload;
    }

    let image = decode_image(handle)?;

    Ok(SourceImage {
        content: ImageContent::Still(image),
        metadata,
        source_format: crate::format::ImageFormat::Heif,
        source_path: path.to_path_buf(),
    })
}

/// Decodes one image handle into an 8-bit [`DynamicImage`].
// verified against docs.rs/libheif-rs 3.0:
// - LibHeif::new() -> LibHeif (Send + Sync guard; one instance per decode)
// - LibHeif::decode(&self, &ImageHandle, ColorSpace, Option<DecodingOptions>)
//   -> Result<Image> (applies irot/imir/crop transformations)
// - Image::planes(&self) -> Planes<&[u8]>; Planes.interleaved: Option<Plane>
// - Plane { data: &[u8], width: u32, height: u32, stride: usize, .. }
fn decode_image(handle: &ImageHandle) -> Result<DynamicImage, Error> {
    // HDR sources (10/12-bit) are down-converted by requesting an 8-bit
    // colorspace; libheif performs the conversion.
    let bits = handle.luma_bits_per_pixel();
    if bits > 8 {
        println!("Warning: HEIC/HEIF image is {bits}-bit; down-converting to 8-bit");
    }

    // There is no convert_to_rgba8 helper: the interleaved chroma is
    // requested at decode time. `RgbChroma::Rgb`/`RgbChroma::Rgba` are the
    // interleaved 8-bit layouts (RGB: 3 bytes/pixel, RGBA: 4 bytes/pixel).
    let chroma = if handle.has_alpha_channel() {
        RgbChroma::Rgba
    } else {
        RgbChroma::Rgb
    };

    let lib_heif = LibHeif::new();
    let heif_image = lib_heif
        .decode(handle, ColorSpace::Rgb(chroma), None)
        .map_err(|err| Error::from_string(format!("libheif decoding failed: {err}")))?;

    let planes = heif_image.planes();
    let plane = planes.interleaved.ok_or_else(|| {
        Error::from_string(
            "libheif did not return an interleaved RGB(A) plane for the decoded image".to_string(),
        )
    })?;

    let (width, height) = (plane.width, plane.height);
    if width == 0 || height == 0 {
        return Err(Error::from_string(format!(
            "libheif decoded an image with invalid dimensions {width}x{height}"
        )));
    }

    // Rows are strided: copy row-by-row instead of assuming packed data.
    let bytes_per_pixel = match chroma {
        RgbChroma::Rgba => 4usize,
        _ => 3usize,
    };
    let row_len = width as usize * bytes_per_pixel;
    let mut pixels = Vec::with_capacity(row_len * height as usize);
    for row in 0..height as usize {
        let start = row * plane.stride;
        let end = start + row_len;
        if end > plane.data.len() {
            return Err(Error::from_string(format!(
                "libheif plane data is truncated (row {row}: need {end} bytes, got {})",
                plane.data.len()
            )));
        }
        pixels.extend_from_slice(&plane.data[start..end]);
    }

    match chroma {
        RgbChroma::Rgba => RgbaImage::from_raw(width, height, pixels)
            .map(DynamicImage::ImageRgba8)
            .ok_or_else(|| {
                Error::from_string("libheif plane buffer has an unexpected size".to_string())
            }),
        _ => RgbImage::from_raw(width, height, pixels)
            .map(DynamicImage::ImageRgb8)
            .ok_or_else(|| {
                Error::from_string("libheif plane buffer has an unexpected size".to_string())
            }),
    }
}

/// Extracts the first EXIF payload of the container, normalized to the raw
/// TIFF stream convention (see `crate::metadata::normalize_exif_payload`).
// verified against docs.rs/libheif-rs 3.0:
// - metadata_block_ids<T: Into<FourCC>>(&self, &mut [ItemId], T) -> usize
// - metadata(&self, ItemId) -> Result<Vec<u8>>
fn extract_exif(handle: &ImageHandle) -> Option<Vec<u8>> {
    let capacity = handle.number_of_metadata_blocks(b"Exif").max(0) as usize;
    let mut ids: Vec<ItemId> = vec![0; capacity];
    let count = handle.metadata_block_ids(&mut ids, b"Exif");
    for id in ids.into_iter().take(count) {
        if let Ok(raw) = handle.metadata(id)
            && let Some(payload) = tiff_payload_from_heif_item(raw)
        {
            return Some(payload);
        }
    }
    None
}

/// Extracts the XMP packet of the container, if present.
// verified against docs.rs/libheif-rs 3.0:
// metadata_content_type(&self, ItemId) -> Option<&str> (XMP items carry
// "application/rdf+xml", EXIF items carry "")
fn extract_xmp(handle: &ImageHandle) -> Option<Vec<u8>> {
    let capacity = handle.number_of_metadata_blocks(b"mime").max(0) as usize;
    let mut ids: Vec<ItemId> = vec![0; capacity];
    let count = handle.metadata_block_ids(&mut ids, b"mime");
    for id in ids.into_iter().take(count) {
        if handle.metadata_content_type(id) == Some(XMP_CONTENT_TYPE)
            && let Ok(data) = handle.metadata(id)
            && !data.is_empty()
        {
            return Some(data);
        }
    }
    None
}

/// Extracts the raw ICC color profile, if the container stores one.
///
/// Images with only an NCLX color box keep `None` (no synthetic profile).
// verified against docs.rs/libheif-rs 3.0:
// color_profile_raw(&self) -> Option<ColorProfileRaw>, ColorProfileRaw {
// data: Vec<u8> } (None when the profile is nclx-only)
fn extract_icc(handle: &ImageHandle) -> Option<Vec<u8>> {
    let profile = handle.color_profile_raw()?;
    let data = profile.data;
    if data.is_empty() { None } else { Some(data) }
}

/// HEIF stores EXIF items as a 4-byte big-endian `exif_tiff_header_offset`
/// prefix followed by the TIFF stream (ISO/IEC 23008-12). An offset of 0
/// means the TIFF header directly follows the prefix field; a non-zero
/// offset points at the header start relative to the item's beginning.
/// Some writers instead embed the JPEG-style `Exif\0\0` marker or a bare
/// TIFF stream — all are normalized here.
fn tiff_payload_from_heif_item(raw: Vec<u8>) -> Option<Vec<u8>> {
    let start = if raw.len() >= 8 {
        let offset = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        match offset {
            0 => 4,
            offset if (4..raw.len()).contains(&offset) => offset,
            // no plausible offset prefix: treat the item as a bare TIFF stream
            _ => 0,
        }
    } else {
        0
    };
    crate::metadata::normalize_exif_payload(raw[start..].to_vec())
}

/// Removes the EXIF Orientation tag from a raw TIFF payload because libheif
/// already baked the transform into the pixels.
///
/// Returns the payload and whether the tag could be removed. On failure the
/// original payload is kept verbatim and `false` is returned: the caller
/// then reports `exif_applied_orientation = false` so the pipeline does not
/// apply the rotation a second time (a warning about the possible double
/// rotation is printed here).
#[cfg(feature = "exif")]
fn strip_orientation_tag(payload: Vec<u8>) -> (Option<Vec<u8>>, bool) {
    use exif::experimental::Writer;

    let parsed = match crate::metadata::exif::parse(&payload) {
        Ok(parsed) => parsed,
        Err(err) => {
            println!(
                "Warning: could not parse the EXIF payload of the HEIC/HEIF image to remove its \
                 Orientation tag ({err}); keeping the tag, the rotation was already applied \
                 during decoding and applying it again would distort the output"
            );
            return (Some(payload), false);
        }
    };

    if crate::metadata::exif::orientation_value(&parsed).is_none() {
        // nothing to strip
        return (Some(payload), true);
    }

    // re-serialize all fields except the (primary) Orientation tag,
    // mirroring `metadata::exif::serialize_all`
    let mut writer = Writer::new();
    let mut seen: Vec<(exif::In, exif::Tag)> = Vec::new();
    for field in parsed.fields() {
        if field.tag == exif::Tag::Orientation && field.ifd_num == exif::In::PRIMARY {
            continue;
        }
        let key = (field.ifd_num, field.tag);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        writer.push_field(field);
    }
    let mut serialized = std::io::Cursor::new(Vec::new());
    match writer.write(&mut serialized, parsed.little_endian()) {
        Ok(()) if !serialized.get_ref().is_empty() => (Some(serialized.into_inner()), true),
        Ok(()) => (Some(payload), false),
        Err(err) => {
            println!(
                "Warning: could not re-serialize the EXIF payload without its Orientation tag \
                 ({err}); keeping the tag, the rotation was already applied during decoding and \
                 applying it again would distort the output"
            );
            (Some(payload), false)
        }
    }
}

/// Best-effort extraction of a panic message.
fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HEIF decode tests need the native libheif + codec libraries and are
    /// therefore run in CI only (gnu/docker images build with `dec-heif`);
    /// locally verifiable behavior lives in `tests/heif_input.rs`.
    #[test]
    fn exif_item_offset_prefix_is_normalized() {
        // tiff_header_offset = 0: TIFF stream directly after the 4-byte field
        let mut item = vec![0, 0, 0, 0];
        item.extend_from_slice(b"II*\0\x08\0\0\0\0\0");
        assert_eq!(
            tiff_payload_from_heif_item(item).as_deref(),
            Some(&b"II*\0\x08\0\0\0\0\0"[..])
        );

        // offset 8: two padding bytes between the field and the TIFF stream
        let mut item = vec![0, 0, 0, 8, 0, 0, 0xEE, 0xEE];
        item.extend_from_slice(b"II*\0\x08\0\0\0\0\0");
        assert_eq!(
            tiff_payload_from_heif_item(item).as_deref(),
            Some(&b"II*\0\x08\0\0\0\0\0"[..])
        );

        // bare TIFF stream without offset prefix
        assert_eq!(
            tiff_payload_from_heif_item(b"MM\0*\0\0\0\0".to_vec()).as_deref(),
            Some(&b"MM\0*\0\0\0\0"[..])
        );

        // JPEG-style `Exif\0\0` marker right after the offset field
        let mut item = vec![0, 0, 0, 0];
        item.extend_from_slice(b"Exif\0\0II*\0");
        assert_eq!(
            tiff_payload_from_heif_item(item).as_deref(),
            Some(&b"II*\0"[..])
        );

        // empty payload normalizes to None; a bare offset field without TIFF
        // stream is passed through (parsing it later yields a warning, no
        // crash) — normalize_exif_payload only drops truly empty payloads
        assert_eq!(tiff_payload_from_heif_item(Vec::new()), None);
        assert_eq!(
            tiff_payload_from_heif_item(vec![0, 0, 0, 0]),
            Some(vec![0, 0, 0, 0])
        );
    }
}
