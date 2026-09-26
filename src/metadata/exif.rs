//! EXIF payload parsing, policy-driven re-serialization and orientation
//! extraction, built on `kamadak-exif` (imported as `exif`).
//!
//! # Payload convention
//!
//! All functions in this module expect the **raw TIFF byte stream**: the
//! content of a JPEG APP1 segment after its `b"Exif\0\0"` header, the body
//! of a PNG `eXIf` chunk, the WebP `EXIF` chunk body after its optional
//! `b"Exif\0\0"` prefix (see [`crate::metadata::normalize_exif_payload`]).
//! Nothing here accepts or produces the `Exif\0\0` marker prefix.

use exif::experimental::Writer;
use exif::{Context, In, Reader, Tag};

use crate::metadata::ImageMetadata;
use crate::metadata::policy::{ExifPolicy, Ifd, TagSelector};

/// Parses a raw TIFF payload into kamadak's [`Exif`][exif::Exif] structure.
pub fn parse(payload: &[u8]) -> Result<exif::Exif, exif::Error> {
    Reader::new().read_raw(payload.to_vec())
}

/// Parses the EXIF attributes of a whole container file (TIFF and friends;
/// kamadak also handles JPEG/PNG/WebP/HEIF containers).
///
/// Used by the extraction fallbacks; returns `None` when the container does
/// not carry EXIF or cannot be parsed.
#[must_use]
pub fn parse_with_container(bytes: &[u8]) -> Option<exif::Exif> {
    Reader::new()
        .read_from_container(&mut std::io::Cursor::new(bytes))
        .ok()
}

/// Returns the EXIF Orientation value (1–8) carried by the parsed fields.
///
/// `None` if the payload has no usable Orientation tag.
#[must_use]
pub fn orientation_value(exif: &exif::Exif) -> Option<u8> {
    exif.get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| (1..=8).contains(value))
}

/// Parses a raw TIFF payload and returns its Orientation value (1–8).
#[must_use]
pub fn orientation_from_payload(payload: &[u8]) -> Option<u8> {
    parse(payload)
        .ok()
        .and_then(|exif| orientation_value(&exif))
}

/// Resolves the EXIF policy against the metadata of a source image.
///
/// This is the single function encoders/pipeline rely on. Rules:
/// 1. No source EXIF → `None` regardless of policy (nothing is synthesized).
/// 2. [`ExifPolicy::Strip`] → `None`.
/// 3. [`ExifPolicy::Keep`] → the original payload, verbatim.
/// 4. [`ExifPolicy::FilterExcept`]/[`ExifPolicy::KeepOnly`] → a re-serialized
///    payload holding the surviving fields; on writer failure a per-file
///    warning is printed and the original payload is kept verbatim (never
///    silently lose everything).
#[must_use]
pub fn resolve(policy: &ExifPolicy, meta: &ImageMetadata) -> Option<Vec<u8>> {
    let payload = meta.exif.as_ref()?;
    match policy {
        ExifPolicy::Strip => None,
        ExifPolicy::Keep => Some(payload.clone()),
        ExifPolicy::FilterExcept(_) | ExifPolicy::KeepOnly(_) => {
            match parse(payload)
                .map_err(TransformError::from)
                .and_then(|parsed| transform(&parsed, policy))
            {
                Ok(filtered) => Some(filtered),
                Err(message) => {
                    println!(
                        "Warning: could not re-serialize EXIF ({message}); keeping metadata verbatim"
                    );
                    Some(payload.clone())
                }
            }
        }
    }
}

/// Errors of the filter/serialize path (kept simple: string messages).
#[derive(Debug)]
pub struct TransformError(String);

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransformError {}

impl From<exif::Error> for TransformError {
    fn from(err: exif::Error) -> Self {
        TransformError(err.to_string())
    }
}

/// Filters the fields of `exif` according to `policy` and re-serializes the
/// surviving fields into a raw TIFF payload.
///
/// Endianness of the source is preserved. Structural pointer fields
/// (ExifIFDPointer etc.) are re-synthesized by the writer.
pub fn transform(exif: &exif::Exif, policy: &ExifPolicy) -> Result<Vec<u8>, TransformError> {
    let (ExifPolicy::FilterExcept(_) | ExifPolicy::KeepOnly(_)) = policy else {
        return Err(TransformError(
            "transform expects a filter policy (FilterExcept/KeepOnly)".to_string(),
        ));
    };

    let mut seen: Vec<(In, Tag)> = Vec::new();
    let mut writer = Writer::new();
    let mut kept = 0u64;
    let mut dropped = 0u64;
    for field in exif.fields() {
        // the writer rejects duplicate fields; keep the first occurrence
        let key = (field.ifd_num, field.tag);
        if seen.contains(&key) {
            continue;
        }
        if field_is_dropped(field, policy) {
            dropped += 1;
        } else {
            seen.push(key);
            writer.push_field(field);
            kept += 1;
        }
    }

    let mut serialized = std::io::Cursor::new(Vec::new());
    writer
        .write(&mut serialized, exif.little_endian())
        .map_err(TransformError::from)?;
    println!(
        "exif: {kept} kept, {dropped} dropped ({})",
        policy.describe()
    );
    Ok(serialized.into_inner())
}

/// Re-serializes every field of `exif` into a raw TIFF payload.
///
/// Used by the extraction fallbacks (e.g. TIFF containers where the decoder
/// exposes no raw payload): the result is a compact, self-contained payload
/// instead of the whole original file.
pub fn serialize_all(exif: &exif::Exif) -> Result<Vec<u8>, TransformError> {
    let mut writer = Writer::new();
    let mut seen: Vec<(In, Tag)> = Vec::new();
    for field in exif.fields() {
        let key = (field.ifd_num, field.tag);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        writer.push_field(field);
    }
    let mut serialized = std::io::Cursor::new(Vec::new());
    writer
        .write(&mut serialized, exif.little_endian())
        .map_err(TransformError::from)?;
    Ok(serialized.into_inner())
}

/// Whether the field is removed under the given filter policy:
/// [`ExifPolicy::FilterExcept`] drops fields matching any selector,
/// [`ExifPolicy::KeepOnly`] keeps fields matching any selector.
fn field_is_dropped(field: &exif::Field, policy: &ExifPolicy) -> bool {
    let selectors = match policy {
        ExifPolicy::FilterExcept(selectors) => selectors,
        ExifPolicy::KeepOnly(selectors) => {
            return !selectors
                .iter()
                .any(|selector| field_matches_selector(field, selector));
        }
        _ => return false,
    };
    selectors
        .iter()
        .any(|selector| field_matches_selector(field, selector))
}

fn field_matches_selector(field: &exif::Field, selector: &TagSelector) -> bool {
    match selector {
        TagSelector::Name(name) => resolve_name(name).is_some_and(|tag| tag == field.tag),
        TagSelector::Ifd(ifd) => field_in_ifd(field, *ifd),
        TagSelector::TagNum(tag_num, ifd) => {
            field.tag.number() == *tag_num && field_in_ifd_context(field, *ifd)
        }
    }
}

/// Resolves a tag name (case-insensitive) from the static table onto a
/// kamadak tag value.
#[must_use]
pub fn resolve_name(name: &str) -> Option<Tag> {
    crate::metadata::policy::KNOWN_TAGS
        .iter()
        .find(|&&(known, _, _)| known.eq_ignore_ascii_case(name))
        .map(|&(_, ifd, tag_num)| tag_for_group(ifd, tag_num))
}
/// Builds a kamadak tag from the IFD group and tag number of the static table.
#[must_use]
pub fn tag_for_group(ifd: Ifd, tag_num: u16) -> Tag {
    match ifd {
        Ifd::Primary | Ifd::Thumbnail => Tag(Context::Tiff, tag_num),
        Ifd::Exif => Tag(Context::Exif, tag_num),
        Ifd::Gps => Tag(Context::Gps, tag_num),
        Ifd::Interop => Tag(Context::Interop, tag_num),
    }
}

fn field_in_ifd(field: &exif::Field, ifd: Ifd) -> bool {
    match ifd {
        Ifd::Primary => field.tag.context() == Context::Tiff && field.ifd_num == In::PRIMARY,
        Ifd::Thumbnail => field.tag.context() == Context::Tiff && field.ifd_num == In::THUMBNAIL,
        Ifd::Exif | Ifd::Gps | Ifd::Interop => field_in_ifd_context(field, ifd),
    }
}

fn field_in_ifd_context(field: &exif::Field, ifd: Ifd) -> bool {
    let context = match ifd {
        Ifd::Primary | Ifd::Thumbnail => Context::Tiff,
        Ifd::Exif => Context::Exif,
        Ifd::Gps => Context::Gps,
        Ifd::Interop => Context::Interop,
    };
    if field.tag.context() != context {
        return false;
    }
    match ifd {
        Ifd::Primary => field.ifd_num == In::PRIMARY,
        Ifd::Thumbnail => field.ifd_num == In::THUMBNAIL,
        // sub-IFDs share the top-level IFD number of their pointer
        Ifd::Exif | Ifd::Gps | Ifd::Interop => true,
    }
}
